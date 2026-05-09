// =====================================================================
// sync_clock.rs
// ---------------------------------------------------------------------
// Lets *anyone* tick the on-chain clock, primarily so the off-clock
// player (or a watching bot / overlay automation account) can flag the
// player on the move when their time runs out without waiting for them
// to volunteer the call.
//
// If the moving side has no time left we finalise as a timeout. Otherwise
// we emit a `ClockUpdated` event so listening overlays can repaint
// without polling.
// =====================================================================

use anchor_lang::prelude::*;

use crate::errors::ChessError;
use crate::events::ClockUpdated;
use crate::instructions::{finalize_game, settle_wager};
use crate::state::{Game, GameEndReason, GameResult, GameStatus};

#[derive(Accounts)]
pub struct SyncClock<'info> {
    /// Anyone may call this — chess clocks are public state. Charging
    /// the caller for compute is sufficient anti-spam protection.
    pub caller: Signer<'info>,

    #[account(
        mut,
        seeds = [b"game", game.creator.as_ref(), &game.session_id.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,

    /// CHECK: PDA-validated vault.
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump = game.wager_vault_bump,
    )]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: settlement recipient.
    #[account(mut, address = game.white)]
    pub white: UncheckedAccount<'info>,

    /// CHECK: settlement recipient.
    #[account(mut, address = game.black)]
    pub black: UncheckedAccount<'info>,
}

pub fn handler(ctx: Context<SyncClock>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    require!(game.status == GameStatus::Active, ChessError::InvalidGameStatus);
    if game.time_control.initial_ms == 0 {
        // Casual game with no clock — nothing to sync.
        return Ok(());
    }

    let now = Clock::get()?.unix_timestamp;
    let elapsed_ms = (now - game.last_move_ts).max(0) as u64 * 1000;

    let mover_clock = if game.chess.side_to_move == 0 {
        game.white_clock_ms
    } else {
        game.black_clock_ms
    };

    if elapsed_ms >= mover_clock {
        // Flag fall: opponent wins.
        let result = if game.chess.side_to_move == 0 {
            GameResult::BlackWins
        } else {
            GameResult::WhiteWins
        };
        if game.chess.side_to_move == 0 {
            game.white_clock_ms = 0;
        } else {
            game.black_clock_ms = 0;
        }
        settle_wager(
            game,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.white.to_account_info(),
            &ctx.accounts.black.to_account_info(),
            result,
        )?;
        finalize_game(game, result, GameEndReason::Timeout, now)?;
        return Ok(());
    }

    emit!(ClockUpdated {
        game: game.key(),
        white_clock_ms: game.white_clock_ms.saturating_sub(if game.chess.side_to_move == 0 {
            elapsed_ms
        } else {
            0
        }),
        black_clock_ms: game.black_clock_ms.saturating_sub(if game.chess.side_to_move == 1 {
            elapsed_ms
        } else {
            0
        }),
        side_to_move: game.chess.side_to_move,
        at: now,
    });
    Ok(())
}
