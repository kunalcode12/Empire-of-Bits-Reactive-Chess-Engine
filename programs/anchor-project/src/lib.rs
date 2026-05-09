// =====================================================================
// Reactive Multiplayer Chess Engine — Solana / Anchor
// ---------------------------------------------------------------------
// This program is the on-chain source of truth for a livestream-native
// chess protocol. It is deliberately built as INFRASTRUCTURE, not as a
// turnkey app: indexers, overlays, AI commentators, Twitch/Discord bots
// and tournament platforms all consume the same event stream.
//
// Module layout:
//
//   chess/         pure chess engine (board, movegen, status detection)
//   instructions/  one file per instruction handler
//   events.rs      every reactive event clients can subscribe to
//   errors.rs      stable, numbered error codes
//   state.rs       Game, StreamSession, Wager vault PDAs
//
// Subscription pathways for clients:
//
//   * `connection.onLogs(programId, "confirmed")` + `BorshCoder.events`
//     — works on any RPC, zero infra.
//   * Helius enhanced webhooks / websockets, with `programIds`
//     filtering on this program's id, give per-event push delivery.
//   * Geyser plug-in -> Kafka stream is recommended for high-throughput
//     analytics pipelines.
//
// See `tests/anchor-project.ts` and `app/` for end-to-end examples.
// =====================================================================

use anchor_lang::prelude::*;

pub mod chess;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("3mCUQGNrzN42qngYE9k6qcF6GqZHMRYJ4MA9TTYcBCdu");

#[program]
pub mod anchor_project {
    use super::*;

    /// Create a new chess match. The signer becomes white. Optionally
    /// escrows a wager and registers a stream session in the same tx.
    pub fn create_game(ctx: Context<CreateGame>, args: CreateGameArgs) -> Result<()> {
        instructions::create_game::handler(ctx, args)
    }

    /// Join an open match as black. Wager-matching is enforced.
    pub fn join_game(ctx: Context<JoinGame>) -> Result<()> {
        instructions::join_game::handler(ctx)
    }

    /// Submit a move. Validated on-chain — illegal moves are rejected
    /// without state mutation. Emits MovePlayed plus check/mate/draw
    /// events as applicable, and finalises the game on terminal states.
    pub fn make_move(ctx: Context<MakeMove>, args: MakeMoveArgs) -> Result<()> {
        instructions::make_move::handler(ctx, args)
    }

    /// Either player resigns. Wager goes entirely to the opponent.
    pub fn resign_game(ctx: Context<ResignGame>) -> Result<()> {
        instructions::resign_game::handler(ctx)
    }

    /// Offer a draw. The offer auto-cancels on the next move.
    pub fn offer_draw(ctx: Context<OfferDraw>) -> Result<()> {
        instructions::offer_draw::handler(ctx)
    }

    /// Counterparty accepts an outstanding draw offer. Wager split 50/50.
    pub fn accept_draw(ctx: Context<AcceptDraw>) -> Result<()> {
        instructions::accept_draw::handler(ctx)
    }

    /// Anyone can tick the clock — used by overlays / bots / opponents
    /// to flag a timed-out player without waiting for them to move.
    pub fn sync_clock(ctx: Context<SyncClock>) -> Result<()> {
        instructions::sync_clock::handler(ctx)
    }

    /// Spectator emote / prediction / comment / clip-marker. Cheap by
    /// design — primary surface is the emitted event, not on-chain data.
    pub fn record_reaction(ctx: Context<RecordReaction>, args: ReactionArgs) -> Result<()> {
        instructions::record_reaction::handler(ctx, args)
    }

    /// Reclaim rent for a finished (or never-started) match.
    pub fn close_game(ctx: Context<CloseGame>) -> Result<()> {
        instructions::close_game::handler(ctx)
    }
}
