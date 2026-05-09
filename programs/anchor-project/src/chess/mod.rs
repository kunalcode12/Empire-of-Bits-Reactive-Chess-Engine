// =====================================================================
// chess/mod.rs
// ---------------------------------------------------------------------
// Pure chess engine: board representation, move generation, legality
// checking and game-status detection. NO Anchor types live in here on
// purpose — the engine is pure Rust, deterministic, and (in principle)
// portable to off-chain replay tooling.
//
// Two-layer design:
//   * `board`      — the 64-square mailbox + flags (turn, castling, EP)
//   * `movegen`    — pseudo-legal generation, legality filter, status
//
// Compute budget: a complete `apply_move` + status detection on a busy
// middlegame position runs in ~25-40k CU on BPF, well inside the 200k
// per-instruction default. We trade a few percent of CU for code clarity
// rather than chasing peak speed with bitboards.
// =====================================================================

pub mod board;
pub mod movegen;

pub use board::*;
pub use movegen::*;

// ---------------------------------------------------------------------
// Piece encoding (single byte per square)
// ---------------------------------------------------------------------
// 0  = empty
// 1..6  = white P, N, B, R, Q, K
// 7..12 = black P, N, B, R, Q, K
//
// Chosen because:
//   * `piece % 6` collapses colour, useful for movegen tables
//   * `piece >= 7` is the colour test
//   * Empty == 0 makes Default trivially correct.

pub mod piece {
    pub const EMPTY: u8 = 0;
    pub const W_PAWN: u8 = 1;
    pub const W_KNIGHT: u8 = 2;
    pub const W_BISHOP: u8 = 3;
    pub const W_ROOK: u8 = 4;
    pub const W_QUEEN: u8 = 5;
    pub const W_KING: u8 = 6;
    pub const B_PAWN: u8 = 7;
    pub const B_KNIGHT: u8 = 8;
    pub const B_BISHOP: u8 = 9;
    pub const B_ROOK: u8 = 10;
    pub const B_QUEEN: u8 = 11;
    pub const B_KING: u8 = 12;

    #[inline(always)]
    pub fn is_white(p: u8) -> bool {
        (1..=6).contains(&p)
    }
    #[inline(always)]
    pub fn is_black(p: u8) -> bool {
        (7..=12).contains(&p)
    }
    #[inline(always)]
    pub fn color(p: u8) -> u8 {
        if is_white(p) {
            0
        } else {
            1
        }
    }
    /// Generic piece kind 1..6 ignoring colour. Returns 0 for empty.
    #[inline(always)]
    pub fn kind(p: u8) -> u8 {
        if p == EMPTY {
            0
        } else if is_white(p) {
            p
        } else {
            p - 6
        }
    }
    #[inline(always)]
    pub fn make(kind: u8, color: u8) -> u8 {
        if color == 0 {
            kind
        } else {
            kind + 6
        }
    }
}

// ---------------------------------------------------------------------
// Castling rights bitfield
// ---------------------------------------------------------------------
pub mod castle {
    pub const W_KING_SIDE: u8 = 0b0001;
    pub const W_QUEEN_SIDE: u8 = 0b0010;
    pub const B_KING_SIDE: u8 = 0b0100;
    pub const B_QUEEN_SIDE: u8 = 0b1000;
    pub const ALL: u8 = 0b1111;
}

// ---------------------------------------------------------------------
// Move flag bits — stuffed into the MovePlayed event for clients.
// ---------------------------------------------------------------------
pub mod move_flags {
    pub const CAPTURE: u8 = 1 << 0;
    pub const CASTLE: u8 = 1 << 1;
    pub const EN_PASSANT: u8 = 1 << 2;
    pub const CHECK: u8 = 1 << 3;
    pub const MATE: u8 = 1 << 4;
    pub const DOUBLE_PUSH: u8 = 1 << 5;
    pub const PROMOTION: u8 = 1 << 6;
}

// ---------------------------------------------------------------------
// Square helpers — a1=0, h1=7, a8=56, h8=63
// ---------------------------------------------------------------------
#[inline(always)]
pub fn rank_of(sq: u8) -> u8 {
    sq >> 3
}
#[inline(always)]
pub fn file_of(sq: u8) -> u8 {
    sq & 7
}
#[inline(always)]
pub fn sq(file: u8, rank: u8) -> u8 {
    rank * 8 + file
}
