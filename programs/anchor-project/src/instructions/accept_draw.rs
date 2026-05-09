// =====================================================================
// accept_draw.rs
// ---------------------------------------------------------------------
// Counterparty accepts an outstanding draw offer. Wager is split 50/50
// (with the odd-lamport-on-draw going to black; see `settle_wager`).
// =====================================================================

use anchor_lang::prelude::*;

use crate::errors::ChessError;
use crate::events::{end_reason as ER, DrawEvent};
use crate::instructions::{finalize_game, settle_wager};
use crate::state::{Game, GameEndReason, GameResult, GameStatus};

#[derive(Accounts)]
pub struct AcceptDraw<'info> {
    pub player: Signer<'info>,

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

    /// CHECK: payout recipient.
    #[account(mut, address = game.white)]
    pub white: UncheckedAccount<'info>,

    /// CHECK: payout recipient.
    #[account(mut, address = game.black)]
    pub black: UncheckedAccount<'info>,
}

pub fn handler(ctx: Context<AcceptDraw>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    require!(game.status == GameStatus::Active, ChessError::InvalidGameStatus);

    let offerer = game.draw_offer.ok_or(error!(ChessError::NoDrawOffer))?;
    let player = ctx.accounts.player.key();
    require!(
        player == game.white || player == game.black,
        ChessError::NotAPlayer
    );
    require!(player != offerer, ChessError::CannotAcceptOwnDrawOffer);

    let now = Clock::get()?.unix_timestamp;
    let ply = game.move_history.len() as u16;

    emit!(DrawEvent {
        game: game.key(),
        ply,
        reason: ER::DRAW_AGREED,
    });

    settle_wager(
        game,
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.white.to_account_info(),
        &ctx.accounts.black.to_account_info(),
        GameResult::Draw,
    )?;
    finalize_game(game, GameResult::Draw, GameEndReason::DrawAgreed, now)?;
    Ok(())
}
