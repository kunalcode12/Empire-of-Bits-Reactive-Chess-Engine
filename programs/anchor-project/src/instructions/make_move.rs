// =====================================================================
// make_move.rs
// ---------------------------------------------------------------------
// THE hottest instruction in the protocol. Validates the move via the
// pure chess engine, ticks the clock, persists into the move-history
// ring buffer, emits `MovePlayed` (and conditionally `CheckEvent`,
// `CheckmateEvent`, `DrawEvent`), and finalises the game on terminal
// states.
//
// Anti-replay: the client must echo the current `move_nonce`. If it
// drifts — because of a network retry or a race with another client
// running for the same player — the second submission gets rejected
// with `StaleMoveNonce` so we never apply the same move twice. The
// nonce increments on every successful application.
// =====================================================================

use anchor_lang::prelude::*;

use crate::chess::{apply_move, compute_status, ChessMove, Status};
use crate::errors::ChessError;
use crate::events::{CheckEvent, CheckmateEvent, DrawEvent, MovePlayed, StalemateEvent};
use crate::events::end_reason as ER;
use crate::instructions::{finalize_game, settle_wager};
use crate::state::{Game, GameEndReason, GameResult, GameStatus, MAX_COMMENT_BYTES};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct MakeMoveArgs {
    pub mv: ChessMove,
    /// Anti-replay nonce. Must equal `game.move_nonce` at call time.
    pub expected_nonce: u16,
    /// Optional inline comment for the move (e.g. streamer narration,
    /// AI commentator output). Capped server-side. NOT used in legality
    /// — purely metadata for indexers.
    pub comment: Option<String>,
}

#[derive(Accounts)]
pub struct MakeMove<'info> {
    pub mover: Signer<'info>,

    #[account(
        mut,
        seeds = [b"game", game.creator.as_ref(), &game.session_id.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,

    /// CHECK: System-owned wager vault. Required only when settlement is
    /// possible (i.e. the move could end the game). We accept it on
    /// every call so the client doesn't need branching account lists.
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump = game.wager_vault_bump,
    )]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: White player — receives wager payout if they win/draw.
    #[account(mut, address = game.white)]
    pub white: UncheckedAccount<'info>,

    /// CHECK: Black player — receives wager payout if they win/draw.
    #[account(mut, address = game.black)]
    pub black: UncheckedAccount<'info>,
}

pub fn handler(ctx: Context<MakeMove>, args: MakeMoveArgs) -> Result<()> {
    let game = &mut ctx.accounts.game;
    require!(game.status == GameStatus::Active, ChessError::InvalidGameStatus);

    // --- authority -------------------------------------------------
    let mover = ctx.accounts.mover.key();
    let expected_color: u8 = if mover == game.white {
        0
    } else if mover == game.black {
        1
    } else {
        return err!(ChessError::NotAPlayer);
    };
    require!(
        expected_color == game.chess.side_to_move,
        ChessError::NotYourTurn
    );

    // --- replay protection -----------------------------------------
    require!(
        args.expected_nonce == game.move_nonce,
        ChessError::StaleMoveNonce
    );

    if let Some(c) = &args.comment {
        require!(
            c.as_bytes().len() <= MAX_COMMENT_BYTES,
            ChessError::CommentTooLong
        );
    }

    // --- clock tick ------------------------------------------------
    let now = Clock::get()?.unix_timestamp;
    let elapsed_ms = clock_elapsed_ms(game.last_move_ts, now);
    let (mover_clock, _other_clock) = if expected_color == 0 {
        (game.white_clock_ms, game.black_clock_ms)
    } else {
        (game.black_clock_ms, game.white_clock_ms)
    };
    if game.time_control.initial_ms > 0 && elapsed_ms >= mover_clock {
        // Flagged on the clock — opponent wins by timeout.
        let result = if expected_color == 0 {
            GameResult::BlackWins
        } else {
            GameResult::WhiteWins
        };
        if expected_color == 0 {
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

    // --- run the chess engine --------------------------------------
    let outcome = apply_move(&mut game.chess, args.mv).map_err(|e| {
        use crate::chess::MoveError as ME;
        match e {
            ME::EmptyFromSquare => error!(ChessError::EmptyFromSquare),
            ME::WrongPieceColor => error!(ChessError::WrongPieceColor),
            ME::IllegalShape => error!(ChessError::IllegalMoveShape),
            ME::PathBlocked => error!(ChessError::PathBlocked),
            ME::FriendlyCapture => error!(ChessError::FriendlyCapture),
            ME::KingLeftInCheck => error!(ChessError::KingLeftInCheck),
            ME::PromotionRequired => error!(ChessError::PromotionRequired),
            ME::InvalidPromotion => error!(ChessError::InvalidPromotion),
            ME::IllegalCastle => error!(ChessError::IllegalCastle),
            ME::IllegalEnPassant => error!(ChessError::IllegalEnPassant),
        }
    })?;

    // --- bookkeeping AFTER engine succeeded ------------------------
    if game.move_history.len() < game.move_history.capacity().max(crate::state::MAX_MOVE_HISTORY) {
        game.move_history.push(args.mv.pack());
    } else {
        // Out of history room — finalise as `Abandoned` rather than
        // silently corrupt state. Practically unreachable (256+ plies).
        finalize_game(game, GameResult::Draw, GameEndReason::Abandoned, now)?;
        return err!(ChessError::MoveHistoryOverflow);
    }

    // Clock decrement + Fischer increment.
    if game.time_control.initial_ms > 0 {
        if expected_color == 0 {
            game.white_clock_ms = game
                .white_clock_ms
                .saturating_sub(elapsed_ms)
                .saturating_add(game.time_control.increment_ms);
        } else {
            game.black_clock_ms = game
                .black_clock_ms
                .saturating_sub(elapsed_ms)
                .saturating_add(game.time_control.increment_ms);
        }
    }
    game.last_move_ts = now;
    game.move_nonce = game.move_nonce.wrapping_add(1);
    // Any move outside an active draw window cancels a stale offer.
    game.draw_offer = None;

    let board_hash = game.chess.position_hash();
    let ply = game.move_history.len() as u16;

    // --- emit MovePlayed -------------------------------------------
    emit!(MovePlayed {
        game: game.key(),
        mover,
        color: expected_color,
        ply,
        from_sq: args.mv.from,
        to_sq: args.mv.to,
        piece: outcome.piece,
        captured: outcome.captured,
        promotion: args.mv.promotion,
        flags: outcome.flags,
        white_clock_ms: game.white_clock_ms,
        black_clock_ms: game.black_clock_ms,
        played_at: now,
        board_hash,
    });

    if outcome.is_check && !outcome.is_mate {
        emit!(CheckEvent {
            game: game.key(),
            ply,
            side_in_check: 1 - expected_color,
        });
    }

    // --- terminal-state handling -----------------------------------
    if outcome.is_mate {
        emit!(CheckmateEvent {
            game: game.key(),
            ply,
            winner: expected_color,
        });
        let result = if expected_color == 0 {
            GameResult::WhiteWins
        } else {
            GameResult::BlackWins
        };
        settle_wager(
            game,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.white.to_account_info(),
            &ctx.accounts.black.to_account_info(),
            result,
        )?;
        finalize_game(game, result, GameEndReason::Checkmate, now)?;
        return Ok(());
    }

    if outcome.is_stalemate {
        emit!(StalemateEvent { game: game.key(), ply });
        emit!(DrawEvent { game: game.key(), ply, reason: ER::STALEMATE });
        settle_wager(
            game,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.white.to_account_info(),
            &ctx.accounts.black.to_account_info(),
            GameResult::Draw,
        )?;
        finalize_game(game, GameResult::Draw, GameEndReason::Stalemate, now)?;
        return Ok(());
    }

    // Non-mate terminal draws: 50-move rule and insufficient material.
    if outcome.draw_50_move || outcome.draw_insufficient {
        let reason = if outcome.draw_50_move {
            GameEndReason::FiftyMove
        } else {
            GameEndReason::InsufficientMaterial
        };
        emit!(DrawEvent {
            game: game.key(),
            ply,
            reason: reason.as_event_code(),
        });
        settle_wager(
            game,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.white.to_account_info(),
            &ctx.accounts.black.to_account_info(),
            GameResult::Draw,
        )?;
        finalize_game(game, GameResult::Draw, reason, now)?;
        return Ok(());
    }

    // Defensive: re-run terminal status detection in case the engine
    // missed something exotic (it shouldn't — but cheap insurance and
    // useful for upgrades that add new draw conditions).
    if let Status::FiftyMove | Status::InsufficientMaterial = compute_status(&game.chess) {
        let reason = match compute_status(&game.chess) {
            Status::FiftyMove => GameEndReason::FiftyMove,
            _ => GameEndReason::InsufficientMaterial,
        };
        emit!(DrawEvent {
            game: game.key(),
            ply,
            reason: reason.as_event_code(),
        });
        settle_wager(
            game,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.white.to_account_info(),
            &ctx.accounts.black.to_account_info(),
            GameResult::Draw,
        )?;
        finalize_game(game, GameResult::Draw, reason, now)?;
    }

    Ok(())
}

#[inline]
fn clock_elapsed_ms(last_ts: i64, now: i64) -> u64 {
    let secs = (now - last_ts).max(0);
    (secs as u64).saturating_mul(1000)
}
