// =====================================================================
// join_game.rs
// ---------------------------------------------------------------------
// Black seats themselves at the board, matching any wager and starting
// the clock. Emits `GameStarted` so overlays/bots can flip into "live".
// =====================================================================

use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::errors::ChessError;
use crate::events::GameStarted;
use crate::state::{Game, GameStatus};

#[derive(Accounts)]
pub struct JoinGame<'info> {
    #[account(mut)]
    pub black: Signer<'info>,

    #[account(
        mut,
        seeds = [b"game", game.creator.as_ref(), &game.session_id.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,

    /// CHECK: System-owned wager vault, validated by PDA seeds.
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump = game.wager_vault_bump,
    )]
    pub vault: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<JoinGame>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    require!(
        game.status == GameStatus::WaitingForOpponent,
        ChessError::InvalidGameStatus
    );
    require!(
        game.black == Pubkey::default(),
        ChessError::GameAlreadyFull
    );
    require!(
        ctx.accounts.black.key() != game.white,
        ChessError::SelfPlayForbidden
    );

    // Wager match — black puts up the same amount as white.
    if game.wager_per_side > 0 {
        let cpi = system_program::Transfer {
            from: ctx.accounts.black.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
        };
        system_program::transfer(
            CpiContext::new(ctx.accounts.system_program.to_account_info(), cpi),
            game.wager_per_side,
        )?;
    }

    let now = Clock::get()?.unix_timestamp;
    game.black = ctx.accounts.black.key();
    game.status = GameStatus::Active;
    game.last_move_ts = now;

    emit!(GameStarted {
        game: game.key(),
        white: game.white,
        black: game.black,
        started_at: now,
    });

    Ok(())
}
