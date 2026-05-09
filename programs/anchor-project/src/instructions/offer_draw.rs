// =====================================================================
// offer_draw.rs
// ---------------------------------------------------------------------
// One side proposes a draw. Stored as `Some(player_pubkey)` on the Game
// account; cleared automatically by `make_move` (any move auto-declines)
// or explicitly by `accept_draw`.
// =====================================================================

use anchor_lang::prelude::*;

use crate::errors::ChessError;
use crate::events::DrawOffered;
use crate::state::{Game, GameStatus};

#[derive(Accounts)]
pub struct OfferDraw<'info> {
    pub player: Signer<'info>,

    #[account(
        mut,
        seeds = [b"game", game.creator.as_ref(), &game.session_id.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,
}

pub fn handler(ctx: Context<OfferDraw>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    require!(game.status == GameStatus::Active, ChessError::InvalidGameStatus);

    let player = ctx.accounts.player.key();
    let color = if player == game.white {
        0
    } else if player == game.black {
        1
    } else {
        return err!(ChessError::NotAPlayer);
    };

    game.draw_offer = Some(player);

    emit!(DrawOffered {
        game: game.key(),
        by: player,
        color,
        ply: game.move_history.len() as u16,
    });

    Ok(())
}
