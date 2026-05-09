// =====================================================================
// chess/board.rs
// ---------------------------------------------------------------------
// Mailbox board representation. Held by the on-chain Game account; also
// the input/output of all movegen functions.
//
// Why mailbox over bitboards on Solana:
//   * Anchor serialisation of `[u8; 64]` is cheap and IDL-friendly.
//   * Movegen for ONE legality check (which is all we need per ix) is
//     well inside compute budget — bitboards mainly win for search-heavy
//     workloads (engines doing millions of NPS), not single-move verify.
//   * Mailbox keeps the code teachable and reviewable, which matters for
//     a public protocol where clients re-implement state for replays.
// =====================================================================

use anchor_lang::prelude::*;

use super::piece::*;
use super::*;

/// Compact, Anchor-serialisable board snapshot. Lives inside `Game` —
/// see `state.rs`. A separate `Board` value type is exposed for movegen
/// so we can mutate freely without holding an `&mut Game` reference.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, InitSpace, Debug)]
pub struct ChessState {
    /// 64-square mailbox; index 0 = a1, 63 = h8.
    pub squares: [u8; 64],
    /// 0 = white to move, 1 = black to move.
    pub side_to_move: u8,
    /// Bitfield of `castle::*` constants.
    pub castling_rights: u8,
    /// `Some(sq)` if the previous move was a pawn double-push that
    /// created a valid en-passant target; else `None`.
    pub en_passant: Option<u8>,
    /// 50-move rule counter (in plies). Reset on capture or pawn move.
    pub halfmove_clock: u8,
    /// Increments after black's move. u16 caps at 65k full moves —
    /// astronomically more than any game will ever reach.
    pub fullmove_number: u16,
}

impl ChessState {
    /// Standard chess starting position.
    pub fn initial() -> Self {
        let mut s = [EMPTY; 64];
        // White back rank
        s[0] = W_ROOK;
        s[1] = W_KNIGHT;
        s[2] = W_BISHOP;
        s[3] = W_QUEEN;
        s[4] = W_KING;
        s[5] = W_BISHOP;
        s[6] = W_KNIGHT;
        s[7] = W_ROOK;
        // White pawns
        for f in 0..8 {
            s[(8 + f) as usize] = W_PAWN;
        }
        // Black pawns
        for f in 0..8 {
            s[(48 + f) as usize] = B_PAWN;
        }
        // Black back rank
        s[56] = B_ROOK;
        s[57] = B_KNIGHT;
        s[58] = B_BISHOP;
        s[59] = B_QUEEN;
        s[60] = B_KING;
        s[61] = B_BISHOP;
        s[62] = B_KNIGHT;
        s[63] = B_ROOK;

        Self {
            squares: s,
            side_to_move: 0,
            castling_rights: castle::ALL,
            en_passant: None,
            halfmove_clock: 0,
            fullmove_number: 1,
        }
    }

    #[inline(always)]
    pub fn at(&self, sq: u8) -> u8 {
        self.squares[sq as usize]
    }
    #[inline(always)]
    pub fn set(&mut self, sq: u8, p: u8) {
        self.squares[sq as usize] = p;
    }

    /// Locate the king for `color` (0 = white, 1 = black). Linear scan
    /// is fine — invoked at most a handful of times per legality check
    /// and on a 64-byte cache-friendly array.
    pub fn king_square(&self, color: u8) -> Option<u8> {
        let target = if color == 0 { W_KING } else { B_KING };
        for i in 0..64u8 {
            if self.squares[i as usize] == target {
                return Some(i);
            }
        }
        None
    }

    /// Lightweight Zobrist-style hash for client-side repetition checks.
    /// We don't use this for legality decisions on-chain (no rep history
    /// is stored to keep account size bounded), but we emit it on every
    /// move so indexers can detect threefold repetition off-chain and
    /// clients can verify their local state matches.
    pub fn position_hash(&self) -> u64 {
        // FNV-1a over all state — fast, allocation-free, deterministic.
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in &self.squares {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= self.side_to_move as u64;
        h = h.wrapping_mul(0x100000001b3);
        h ^= self.castling_rights as u64;
        h = h.wrapping_mul(0x100000001b3);
        if let Some(ep) = self.en_passant {
            h ^= ep as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }
}

// ---------------------------------------------------------------------
// Move struct — used by movegen and the `make_move` instruction args.
// ---------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Copy, Clone, Debug, InitSpace, Default)]
pub struct ChessMove {
    pub from: u8,
    pub to: u8,
    /// 0 = none, else 2..5 (knight, bishop, rook, queen) — colour is
    /// implied by the moving side.
    pub promotion: u8,
}

impl ChessMove {
    /// Pack into a u16 for the on-chain history ring buffer.
    /// Layout: `pppp_tttttt_ffffff` (4 bits promo, 6 bits to, 6 bits from)
    pub fn pack(&self) -> u16 {
        ((self.promotion as u16) << 12) | ((self.to as u16) << 6) | (self.from as u16)
    }
    pub fn unpack(packed: u16) -> Self {
        Self {
            from: (packed & 0x3F) as u8,
            to: ((packed >> 6) & 0x3F) as u8,
            promotion: ((packed >> 12) & 0x0F) as u8,
        }
    }
}
