// =====================================================================
// create_game.rs
// ---------------------------------------------------------------------
// Mints a new chess match. The creator becomes white by convention (this
// keeps account derivation simple and is the standard for casual play —
// random colour assignment is left to the matchmaking layer above us).
//
// PDA seeds:
//   game     = ["game",  creator, session_id_le]
//   stream   = ["stream", game]
//   vault    = ["vault",  game]   (system-owned, lamport-only)
//
// The creator funds:
//   * Game account rent
//   * StreamSession account rent
//   * (optionally) wager_per_side lamports → vault
// =====================================================================

use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::chess::ChessState;
use crate::errors::ChessError;
use crate::events::GameCreated;
use crate::state::{Game, GameEndReason, GameResult, GameStatus, StreamSession, TimeControl};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CreateGameArgs {
    pub session_id: u64,
    pub time_control: TimeControl,
    pub wager_per_side: u64,
    pub stream_meta: String,
    pub stream_label: String,
    pub streamer: Option<Pubkey>,
}

#[derive(Accounts)]
#[instruction(args: CreateGameArgs)]
pub struct CreateGame<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,

    #[account(
        init,
        payer = creator,
        space = Game::SPACE,
        seeds = [b"game", creator.key().as_ref(), &args.session_id.to_le_bytes()],
        bump,
    )]
    pub game: Account<'info, Game>,

    #[account(
        init,
        payer = creator,
        space = StreamSession::SPACE,
        seeds = [b"stream", game.key().as_ref()],
        bump,
    )]
    pub stream: Account<'info, StreamSession>,

    /// CHECK: System-owned wager vault. We only ever transfer lamports
    /// in/out and never read its data; safety comes from the PDA seeds.
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump,
    )]
    pub vault: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<CreateGame>, args: CreateGameArgs) -> Result<()> {
    require!(args.time_control.validate(), ChessError::InvalidTimeControl);
    require!(
        args.stream_meta.as_bytes().len() <= crate::state::MAX_STREAM_META,
        ChessError::StreamMetaTooLong
    );
    require!(
        args.stream_label.as_bytes().len() <= crate::state::MAX_STREAM_META,
        ChessError::StreamMetaTooLong
    );

    let now = Clock::get()?.unix_timestamp;

    // --- fund wager vault -----------------------------------------------
    if args.wager_per_side > 0 {
        let cpi = system_program::Transfer {
            from: ctx.accounts.creator.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
        };
        system_program::transfer(
            CpiContext::new(ctx.accounts.system_program.to_account_info(), cpi),
            args.wager_per_side,
        )?;
    }

    // --- populate Game --------------------------------------------------
    let game = &mut ctx.accounts.game;
    game.session_id = args.session_id;
    game.creator = ctx.accounts.creator.key();
    game.white = ctx.accounts.creator.key();
    game.black = Pubkey::default();
    game.streamer = args.streamer;
    game.chess = ChessState::initial();
    game.status = GameStatus::WaitingForOpponent;
    game.result = GameResult::Ongoing;
    game.end_reason = GameEndReason::None;
    game.white_clock_ms = args.time_control.initial_ms;
    game.black_clock_ms = args.time_control.initial_ms;
    game.last_move_ts = now;
    game.time_control = args.time_control;
    game.move_history = Vec::new();
    game.draw_offer = None;
    game.wager_per_side = args.wager_per_side;
    game.wager_vault_bump = ctx.bumps.vault;
    game.stream_meta = args.stream_meta;
    game.spectators = 0;
    game.move_nonce = 0;
    game.created_at = now;
    game.bump = ctx.bumps.game;

    // --- populate StreamSession ----------------------------------------
    let stream = &mut ctx.accounts.stream;
    stream.game = game.key();
    stream.created_at = now;
    stream.reaction_count = 0;
    stream.clip_marker_count = 0;
    stream.label = args.stream_label;
    stream.bump = ctx.bumps.stream;

    emit!(GameCreated {
        game: game.key(),
        creator: game.creator,
        white: game.white,
        session_id: game.session_id,
        initial_time_ms: args.time_control.initial_ms,
        increment_ms: args.time_control.increment_ms,
        wager_lamports: args.wager_per_side,
        created_at: now,
    });

    Ok(())
}
