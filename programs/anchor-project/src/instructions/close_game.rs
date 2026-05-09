// =====================================================================
// close_game.rs
// ---------------------------------------------------------------------
// Reclaims rent for finished (or never-started) games. Either player can
// close after the game has finalised; only the creator can close a game
// that is still `WaitingForOpponent` (and they'll be refunded any wager
// they put up).
//
// Rent destination is the creator by convention — most matchmaking
// flows already hold the creator's pubkey, and routing rent to them
// keeps client UX simple.
// =====================================================================

use anchor_lang::prelude::*;

use crate::errors::ChessError;
use crate::state::{Game, GameStatus, StreamSession};

#[derive(Accounts)]
pub struct CloseGame<'info> {
    #[account(mut)]
    pub closer: Signer<'info>,

    #[account(
        mut,
        seeds = [b"game", game.creator.as_ref(), &game.session_id.to_le_bytes()],
        bump = game.bump,
        close = creator,
    )]
    pub game: Account<'info, Game>,

    #[account(
        mut,
        seeds = [b"stream", game.key().as_ref()],
        bump = stream.bump,
        close = creator,
        has_one = game,
    )]
    pub stream: Account<'info, StreamSession>,

    /// CHECK: PDA-validated wager vault. Drained to creator on close.
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump = game.wager_vault_bump,
    )]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: rent recipient (the original creator).
    #[account(mut, address = game.creator)]
    pub creator: UncheckedAccount<'info>,
}

pub fn handler(ctx: Context<CloseGame>) -> Result<()> {
    let game = &ctx.accounts.game;
    let closer = ctx.accounts.closer.key();

    match game.status {
        GameStatus::WaitingForOpponent => {
            // Only the creator may unilaterally close an unstarted game,
            // and they get any wager they put up back via the vault drain.
            require!(closer == game.creator, ChessError::NotCreator);
        }
        GameStatus::Finished => {
            require!(
                closer == game.white || closer == game.black || closer == game.creator,
                ChessError::NotAPlayer
            );
        }
        GameStatus::Active => {
            // Active games must be resigned/drawn/checkmated first.
            return err!(ChessError::InvalidGameStatus);
        }
    }

    // Drain any remaining lamports from the wager vault to the creator
    // (settlement should have already paid out to the winner; anything
    // left is rent dust or a refund for an unstarted match).
    let remaining = ctx.accounts.vault.lamports();
    if remaining > 0 {
        **ctx.accounts.vault.try_borrow_mut_lamports()? = 0;
        **ctx.accounts.creator.try_borrow_mut_lamports()? = ctx
            .accounts
            .creator
            .lamports()
            .checked_add(remaining)
            .ok_or(ChessError::MathOverflow)?;
    }

    Ok(())
}
