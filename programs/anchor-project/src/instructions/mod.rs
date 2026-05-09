// =====================================================================
// instructions/mod.rs
// ---------------------------------------------------------------------
// One file per instruction. Keeps each handler small enough to audit on
// its own and minimises merge conflicts when the team is shipping new
// reactive features in parallel.
// =====================================================================

pub mod accept_draw;
pub mod close_game;
pub mod create_game;
pub mod join_game;
pub mod make_move;
pub mod offer_draw;
pub mod record_reaction;
pub mod resign_game;
pub mod sync_clock;

// Glob re-exports are required so that Anchor's `#[program]` macro in
// `lib.rs` can resolve each Accounts context AND the auto-generated
// `__client_accounts_*` / `__cpi_client_accounts_*` helper modules.
// The `handler` function name collides across modules but lib.rs always
// calls them by their full path (`instructions::create_game::handler`),
// so the ambiguity is harmless — silence the warning.
#[allow(ambiguous_glob_reexports)]
pub use accept_draw::*;
#[allow(ambiguous_glob_reexports)]
pub use close_game::*;
#[allow(ambiguous_glob_reexports)]
pub use create_game::*;
#[allow(ambiguous_glob_reexports)]
pub use join_game::*;
#[allow(ambiguous_glob_reexports)]
pub use make_move::*;
#[allow(ambiguous_glob_reexports)]
pub use offer_draw::*;
#[allow(ambiguous_glob_reexports)]
pub use record_reaction::*;
#[allow(ambiguous_glob_reexports)]
pub use resign_game::*;
#[allow(ambiguous_glob_reexports)]
pub use sync_clock::*;

// =====================================================================
// Internal helpers shared across instructions.
// =====================================================================

use crate::events::{end_reason, GameEnded};
use crate::state::{Game, GameEndReason, GameResult, GameStatus};
use anchor_lang::prelude::*;

/// Finalise the game with a given outcome and emit `GameEnded`. Caller
/// is responsible for any additional bookkeeping (wager settlement, draw
/// offer cleanup) before invoking this.
pub fn finalize_game(
    game: &mut Account<Game>,
    result: GameResult,
    reason: GameEndReason,
    now: i64,
) -> Result<()> {
    game.status = GameStatus::Finished;
    game.result = result;
    game.end_reason = reason;
    game.draw_offer = None;

    let result_code: u8 = match result {
        GameResult::Ongoing => 0,
        GameResult::WhiteWins => 1,
        GameResult::BlackWins => 2,
        GameResult::Draw => 3,
    };

    emit!(GameEnded {
        game: game.key(),
        result: result_code,
        reason: reason.as_event_code(),
        final_ply: game.move_history.len() as u16,
        ended_at: now,
    });
    let _ = end_reason::CHECKMATE; // keep the const linked when otherwise unused.
    Ok(())
}

/// Settle a wager from the vault to a winning player (or split for a
/// draw). Pulled out into its own helper so it's reviewable in one place.
pub fn settle_wager<'info>(
    game: &Account<'info, Game>,
    vault: &AccountInfo<'info>,
    white: &AccountInfo<'info>,
    black: &AccountInfo<'info>,
    result: GameResult,
) -> Result<()> {
    if game.wager_per_side == 0 {
        return Ok(());
    }
    let pot = game.wager_per_side.saturating_mul(2);
    let vault_lamports = vault.lamports();
    let payout = pot.min(vault_lamports);

    match result {
        GameResult::WhiteWins => {
            transfer_from_vault(vault, white, payout)?;
        }
        GameResult::BlackWins => {
            transfer_from_vault(vault, black, payout)?;
        }
        GameResult::Draw => {
            let half = payout / 2;
            transfer_from_vault(vault, white, half)?;
            // Send the remainder (covers odd-lamport pots) to black.
            transfer_from_vault(vault, black, payout - half)?;
        }
        GameResult::Ongoing => {} // no-op; ongoing shouldn't reach here
    }
    Ok(())
}

fn transfer_from_vault<'info>(
    vault: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    lamports: u64,
) -> Result<()> {
    if lamports == 0 {
        return Ok(());
    }
    // Direct lamport accounting works because the vault is a system-owned
    // account with zero data — no rent-exempt floor to preserve since we
    // close the vault on `close_game`.
    **vault.try_borrow_mut_lamports()? = vault
        .lamports()
        .checked_sub(lamports)
        .ok_or(crate::errors::ChessError::WagerVaultUnderfunded)?;
    **to.try_borrow_mut_lamports()? = to
        .lamports()
        .checked_add(lamports)
        .ok_or(crate::errors::ChessError::MathOverflow)?;
    Ok(())
}
