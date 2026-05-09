// =====================================================================
// state.rs
// ---------------------------------------------------------------------
// Canonical on-chain accounts for the Reactive Chess protocol.
//
// Three account types:
//
//   1. `Game`           — chess state, players, clocks, wager, history.
//   2. `StreamSession`  — high-frequency spectator/reaction surface that
//                         is split out so reactions don't bloat `Game`
//                         and so it can be closed independently.
//   3. `WagerVault`     — system-owned PDA that escrows lamports for the
//                         match (only created if `wager_lamports > 0`).
//
// All three are PDAs — no signer keypair is ever needed off the player /
// streamer pair, which makes the protocol trivially programmable and
// safe against impersonation.
//
// We use Borsh-derived `#[account]` (not `#[account(zero_copy)]`) for
// `Game` because the move-history `Vec<u16>` does not survive the
// zero-copy bytemuck contract. If you ever need the absolute lowest CU
// for high-frequency tournaments, swap `move_history` for a fixed-size
// array and flip on `#[account(zero_copy)]`.
// =====================================================================

use anchor_lang::prelude::*;

use crate::chess::ChessState;

// ---------------------------------------------------------------------
// Hard limits — chosen so a Game account fits comfortably under 4 KB of
// rent-exempt space while supporting practical match lengths.
// ---------------------------------------------------------------------
pub const MAX_MOVE_HISTORY: usize = 256; // half-moves; > 99% of human games
pub const MAX_STREAM_META: usize = 128;  // bytes; e.g. "twitch:foo|yt:bar"
pub const MAX_REACTION_BYTES: usize = 32;
pub const MAX_COMMENT_BYTES: usize = 96;

// ---------------------------------------------------------------------
// Game-status enum — small because it's serialised per-account.
// ---------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum GameStatus {
    /// Created by white, waiting for black to call `join_game`.
    WaitingForOpponent,
    /// Both colours seated, clocks ticking.
    Active,
    /// Game ended — see `result` and `end_reason`.
    Finished,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum GameResult {
    Ongoing,
    WhiteWins,
    BlackWins,
    Draw,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum GameEndReason {
    None,
    Checkmate,
    Resignation,
    Timeout,
    DrawAgreed,
    Stalemate,
    FiftyMove,
    InsufficientMaterial,
    Abandoned,
}

impl GameEndReason {
    pub fn as_event_code(&self) -> u8 {
        use crate::events::end_reason::*;
        match self {
            GameEndReason::None => 0,
            GameEndReason::Checkmate => CHECKMATE,
            GameEndReason::Resignation => RESIGNATION,
            GameEndReason::Timeout => TIMEOUT,
            GameEndReason::DrawAgreed => DRAW_AGREED,
            GameEndReason::Stalemate => STALEMATE,
            GameEndReason::FiftyMove => FIFTY_MOVE,
            GameEndReason::InsufficientMaterial => INSUFFICIENT_MATERIAL,
            GameEndReason::Abandoned => ABANDONED,
        }
    }
}

// ---------------------------------------------------------------------
// Time control — Fischer-style (initial + per-move increment).
// `delay_ms` is reserved for future Bronstein-delay support; clients
// should treat non-zero values as advisory until a follow-up upgrade
// implements them in `make_move`.
// ---------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, InitSpace)]
pub struct TimeControl {
    pub initial_ms: u64,
    pub increment_ms: u64,
    pub delay_ms: u64,
}

impl TimeControl {
    pub fn validate(&self) -> bool {
        // Allow casual / no-clock games (initial_ms == 0) but reject
        // pathological values that could overflow elapsed-time math.
        self.initial_ms <= 24 * 60 * 60 * 1000
            && self.increment_ms <= 60 * 1000
            && self.delay_ms <= 60 * 1000
    }
}

// ---------------------------------------------------------------------
// Game account — the heart of the protocol.
// ---------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct Game {
    /// Identifier baked into the PDA seed; lets a single creator run many
    /// concurrent matches without nonce collisions.
    pub session_id: u64,

    /// Match creator (usually == white). Permitted to close the game
    /// while it's still `WaitingForOpponent`.
    pub creator: Pubkey,

    /// Player pubkeys. `black` is `Pubkey::default()` until `join_game`.
    pub white: Pubkey,
    pub black: Pubkey,

    /// Optional streamer / broadcaster identity. Used to scope event
    /// filters in indexers ("show me all moves on @hikaru's stream").
    pub streamer: Option<Pubkey>,

    /// Live chess state. Movegen mutates this in place each `make_move`.
    pub chess: ChessState,

    /// Lifecycle flags.
    pub status: GameStatus,
    pub result: GameResult,
    pub end_reason: GameEndReason,

    /// Clocks (milliseconds). Decremented on the moving side's move
    /// based on `last_move_ts`. Held as `u64` to allow > 2 hr time
    /// controls without saturating.
    pub white_clock_ms: u64,
    pub black_clock_ms: u64,
    pub last_move_ts: i64,

    pub time_control: TimeControl,

    /// Compact packed move history (see `ChessMove::pack`). Capped by
    /// `MAX_MOVE_HISTORY`; if a game ever exceeds it we transition to
    /// `Finished` with reason `Abandoned` rather than refuse the move.
    #[max_len(MAX_MOVE_HISTORY)]
    pub move_history: Vec<u16>,

    /// Pending draw offer pubkey (the offerer); `None` if no offer.
    pub draw_offer: Option<Pubkey>,

    /// Wager: lamports each side put up. The full pot lives in
    /// `WagerVault`. `wager_per_side == 0` means casual game.
    pub wager_per_side: u64,
    pub wager_vault_bump: u8,

    /// Free-form metadata string for streamer overlays — e.g. a JSON
    /// blob with platform handles, theme, room id. Capped to
    /// `MAX_STREAM_META` bytes.
    #[max_len(MAX_STREAM_META)]
    pub stream_meta: String,

    /// Spectator headcount for cheap onchain "viewers" badge.
    pub spectators: u32,

    /// Anti-replay: monotonic ply nonce that clients must echo with each
    /// `make_move`. Catches double-submits during reconnects and stops
    /// stale moves from being replayed by a network adversary.
    pub move_nonce: u16,

    pub created_at: i64,
    pub bump: u8,
}

impl Game {
    /// Anchor 8-byte discriminator + InitSpace.
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

// ---------------------------------------------------------------------
// StreamSession — separate PDA so reaction load doesn't fragment Game.
// Heavy-traffic overlays should subscribe to events emitted from this
// account's instructions rather than to writes on the Game itself.
// ---------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct StreamSession {
    pub game: Pubkey,
    pub created_at: i64,
    pub reaction_count: u64,
    pub clip_marker_count: u32,
    /// Human-readable label for indexers ("twitch:hikaru/2026-05-09").
    #[max_len(MAX_STREAM_META)]
    pub label: String,
    pub bump: u8,
}

impl StreamSession {
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

// ---------------------------------------------------------------------
// WagerVault — system-owned (no data) PDA; we just hold lamports and let
// `system_program::transfer` move them around. Settlement is a CPI from
// the program-owned authority (the Game PDA itself, via signer seeds).
// ---------------------------------------------------------------------
//
// We don't define a struct because a system-owned account has no data;
// the seed pattern is `[b"vault", game.key().as_ref()]` and the bump is
// stored on the Game account.
