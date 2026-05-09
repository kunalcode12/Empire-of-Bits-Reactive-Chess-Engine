// =====================================================================
// resign_game.rs
// ---------------------------------------------------------------------
// Either player can resign at any point during an active game. Wager
// (if any) goes entirely to the opponent.
// =====================================================================

use anchor_lang::prelude::*;

use crate::errors::ChessError;
use crate::instructions::{finalize_game, settle_wager};
use crate::state::{Game, GameEndReason, GameResult, GameStatus};

#[derive(Accounts)]
pub struct ResignGame<'info> {
    pub player: Signer<'info>,

    #[account(
        mut,
        seeds = [b"game", game.creator.as_ref(), &game.session_id.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,

    /// CHECK: PDA-validated wager vault.
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump = game.wager_vault_bump,
    )]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: white player; lamports recipient on win.
    #[account(mut, address = game.white)]
    pub white: UncheckedAccount<'info>,

    /// CHECK: black player; lamports recipient on win.
    #[account(mut, address = game.black)]
    pub black: UncheckedAccount<'info>,
}

pub fn handler(ctx: Context<ResignGame>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    require!(game.status == GameStatus::Active, ChessError::InvalidGameStatus);

    let player = ctx.accounts.player.key();
    let result = if player == game.white {
        GameResult::BlackWins
    } else if player == game.black {
        GameResult::WhiteWins
    } else {
        return err!(ChessError::NotAPlayer);
    };

    let now = Clock::get()?.unix_timestamp;
    settle_wager(
        game,
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.white.to_account_info(),
        &ctx.accounts.black.to_account_info(),
        result,
    )?;
    finalize_game(game, result, GameEndReason::Resignation, now)?;
    Ok(())
}
