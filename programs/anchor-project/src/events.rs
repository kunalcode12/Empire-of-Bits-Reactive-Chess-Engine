// =====================================================================
// events.rs
// ---------------------------------------------------------------------
// Structured Anchor events emitted by every state-changing instruction.
// These are the protocol's REACTIVE LAYER — frontends, AI commentators,
// Twitch/Discord bots, Helius webhooks, Geyser plug-ins and overlays
// subscribe to these via either:
//
//   * `connection.onLogs(programId, …)` and parse base64-encoded events
//   * Helius enhanced-websocket "event" stream
//   * Geyser plug-in -> Kafka/Redpanda -> downstream consumers
//
// Schemas are intentionally flat and self-describing: every event carries
// the `game` pubkey, current `ply`, and a unix timestamp so consumers
// can correlate without a second RPC round-trip.
// =====================================================================

use anchor_lang::prelude::*;

// ---------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------

#[event]
pub struct GameCreated {
    pub game: Pubkey,
    pub creator: Pubkey,
    pub white: Pubkey,
    pub session_id: u64,
    pub initial_time_ms: u64,
    pub increment_ms: u64,
    pub wager_lamports: u64,
    pub created_at: i64,
}

#[event]
pub struct GameStarted {
    pub game: Pubkey,
    pub white: Pubkey,
    pub black: Pubkey,
    pub started_at: i64,
}

#[event]
pub struct GameEnded {
    pub game: Pubkey,
    pub result: u8,        // 0 = ongoing, 1 = white, 2 = black, 3 = draw
    pub reason: u8,        // see GameEndReason below
    pub final_ply: u16,
    pub ended_at: i64,
}

/// Mirrors `state::GameEndReason`. Kept as `u8` on the wire so old clients
/// never break when new variants are added — they simply log "unknown".
pub mod end_reason {
    pub const CHECKMATE: u8 = 1;
    pub const RESIGNATION: u8 = 2;
    pub const TIMEOUT: u8 = 3;
    pub const DRAW_AGREED: u8 = 4;
    pub const STALEMATE: u8 = 5;
    pub const FIFTY_MOVE: u8 = 6;
    pub const INSUFFICIENT_MATERIAL: u8 = 7;
    pub const ABANDONED: u8 = 8;
}

// ---------------------------------------------------------------------
// Move + chess events
// ---------------------------------------------------------------------

/// The single most important event in the protocol — emitted on every
/// successful move. Overlays render from this; AI commentators feed it
/// into their pipelines; archival indexers persist it for replay.
#[event]
pub struct MovePlayed {
    pub game: Pubkey,
    pub mover: Pubkey,
    pub color: u8,         // 0 = white, 1 = black
    pub ply: u16,
    pub from_sq: u8,       // 0..63, a1=0, h8=63
    pub to_sq: u8,
    pub piece: u8,          // see chess::piece::*
    pub captured: u8,       // 0 if no capture
    pub promotion: u8,      // 0 if no promo, else 2..5 (N/B/R/Q)
    pub flags: u8,          // bit0=capture, bit1=castle, bit2=ep, bit3=check, bit4=mate, bit5=double-push
    pub white_clock_ms: u64,
    pub black_clock_ms: u64,
    pub played_at: i64,
    pub board_hash: u64,    // Zobrist-style hash for client-side rep checks
}

#[event]
pub struct CheckEvent {
    pub game: Pubkey,
    pub ply: u16,
    pub side_in_check: u8,   // 0 = white, 1 = black
}

#[event]
pub struct CheckmateEvent {
    pub game: Pubkey,
    pub ply: u16,
    pub winner: u8,           // 0 = white, 1 = black
}

#[event]
pub struct StalemateEvent {
    pub game: Pubkey,
    pub ply: u16,
}

#[event]
pub struct DrawEvent {
    pub game: Pubkey,
    pub ply: u16,
    pub reason: u8,           // end_reason::DRAW_AGREED | FIFTY_MOVE | INSUFFICIENT_MATERIAL | STALEMATE
}

// ---------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------

#[event]
pub struct ClockUpdated {
    pub game: Pubkey,
    pub white_clock_ms: u64,
    pub black_clock_ms: u64,
    pub side_to_move: u8,
    pub at: i64,
}

// ---------------------------------------------------------------------
// Draw flow
// ---------------------------------------------------------------------

#[event]
pub struct DrawOffered {
    pub game: Pubkey,
    pub by: Pubkey,
    pub color: u8,
    pub ply: u16,
}

#[event]
pub struct DrawDeclined {
    pub game: Pubkey,
    pub by: Pubkey,
    pub ply: u16,
}

// ---------------------------------------------------------------------
// Stream / audience reactivity
// ---------------------------------------------------------------------

/// Surface for spectators / bots / overlays to attach a reaction to a
/// specific ply. Designed to be high-frequency and cheap — payload is
/// capped on the program side so bots can't spam the indexer.
#[event]
pub struct StreamReactionEvent {
    pub game: Pubkey,
    pub spectator: Pubkey,
    pub ply: u16,
    pub kind: u8,              // 0 = emote, 1 = prediction, 2 = comment, 3 = clip-marker
    pub payload: [u8; 32],     // emote shortcode, predicted square, etc — free-form 32 bytes
    pub at: i64,
}

#[event]
pub struct SpectatorJoined {
    pub game: Pubkey,
    pub spectator: Pubkey,
    pub total_spectators: u32,
}
