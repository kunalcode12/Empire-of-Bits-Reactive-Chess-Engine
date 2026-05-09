// =====================================================================
// record_reaction.rs
// ---------------------------------------------------------------------
// The "Twitch chat lever" of the protocol. Spectators (or bots, or AI
// agents, or overlay services) attach a reaction to the current ply.
//
// We DON'T persist the reaction list in StreamSession — that would
// force monotonic account growth on a hot path. Instead we emit a
// `StreamReactionEvent` that downstream indexers (Helius webhooks,
// Geyser plug-ins, your own websocket listener) capture in real time.
// The on-chain footprint is just two counters bumped in StreamSession,
// useful for cheap "reactions per game" badges.
//
// `kind` byte:
//   0 = emote shortcode (payload = utf-8, padded with 0)
//   1 = audience prediction (payload[0..2] = predicted from-sq, to-sq)
//   2 = comment (payload = first 32 bytes of utf-8 — full text in
//                an off-chain store keyed by tx signature)
//   3 = clip marker (payload = 32 bytes of free-form clip metadata)
// =====================================================================

use anchor_lang::prelude::*;

use crate::errors::ChessError;
use crate::events::{SpectatorJoined, StreamReactionEvent};
use crate::state::{Game, StreamSession, MAX_REACTION_BYTES};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ReactionArgs {
    pub kind: u8,
    pub payload: [u8; MAX_REACTION_BYTES],
    /// True iff this is the spectator's first interaction in this
    /// session. Lets bots and overlays opportunistically increment the
    /// viewer counter without a separate `register_spectator` ix.
    pub register_as_spectator: bool,
}

#[derive(Accounts)]
pub struct RecordReaction<'info> {
    pub spectator: Signer<'info>,

    #[account(
        mut,
        seeds = [b"game", game.creator.as_ref(), &game.session_id.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,

    #[account(
        mut,
        seeds = [b"stream", game.key().as_ref()],
        bump = stream.bump,
        has_one = game,
    )]
    pub stream: Account<'info, StreamSession>,
}

pub fn handler(ctx: Context<RecordReaction>, args: ReactionArgs) -> Result<()> {
    require!(args.kind <= 3, ChessError::ReactionTooLong);
    let now = Clock::get()?.unix_timestamp;
    let stream = &mut ctx.accounts.stream;
    let game = &mut ctx.accounts.game;

    stream.reaction_count = stream.reaction_count.saturating_add(1);
    if args.kind == 3 {
        stream.clip_marker_count = stream.clip_marker_count.saturating_add(1);
    }

    if args.register_as_spectator {
        game.spectators = game.spectators.saturating_add(1);
        emit!(SpectatorJoined {
            game: game.key(),
            spectator: ctx.accounts.spectator.key(),
            total_spectators: game.spectators,
        });
    }

    emit!(StreamReactionEvent {
        game: game.key(),
        spectator: ctx.accounts.spectator.key(),
        ply: game.move_history.len() as u16,
        kind: args.kind,
        payload: args.payload,
        at: now,
    });

    Ok(())
}
