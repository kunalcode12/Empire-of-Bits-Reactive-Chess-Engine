// =====================================================================
// chess/movegen.rs
// ---------------------------------------------------------------------
// Pseudo-legal generation, full legality filtering, special-move handling
// (castling, en-passant, promotion) and game-status detection.
//
// The public surface is intentionally tiny:
//
//   * `apply_move(state, mv) -> Result<MoveOutcome, MoveError>`
//     -- the *only* function the on-chain `make_move` instruction calls
//   * `compute_status(state) -> Status`
//     -- called after `apply_move` to detect mate/stalemate/etc
//
// Everything else is private machinery so we can swap representations
// later (e.g. drop in bitboards) without breaking the program API.
// =====================================================================

use super::piece::*;
use super::*;

// ---------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveError {
    EmptyFromSquare,
    WrongPieceColor,
    IllegalShape,
    PathBlocked,
    FriendlyCapture,
    KingLeftInCheck,
    PromotionRequired,
    InvalidPromotion,
    IllegalCastle,
    IllegalEnPassant,
}

#[derive(Debug, Clone, Copy)]
pub struct MoveOutcome {
    pub piece: u8,
    pub captured: u8, // 0 if no capture
    pub flags: u8,
    pub is_check: bool,
    pub is_mate: bool,
    pub is_stalemate: bool,
    pub draw_50_move: bool,
    pub draw_insufficient: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ongoing,
    Checkmate { winner: u8 },
    Stalemate,
    InsufficientMaterial,
    FiftyMove,
}

// ---------------------------------------------------------------------
// Top-level: apply a move to the live state, returning a rich outcome
// (or an error explaining why the move was illegal). This function is
// the SINGLE point of trust for chess legality on-chain.
// ---------------------------------------------------------------------

pub fn apply_move(state: &mut ChessState, mv: ChessMove) -> Result<MoveOutcome, MoveError> {
    let from = mv.from;
    let to = mv.to;
    if from >= 64 || to >= 64 || from == to {
        return Err(MoveError::IllegalShape);
    }

    let piece = state.at(from);
    if piece == EMPTY {
        return Err(MoveError::EmptyFromSquare);
    }
    if color(piece) != state.side_to_move {
        return Err(MoveError::WrongPieceColor);
    }

    let target = state.at(to);
    if target != EMPTY && color(target) == state.side_to_move {
        return Err(MoveError::FriendlyCapture);
    }

    // Categorise + validate by piece kind, computing whatever side-effects
    // (castling rook hop, en-passant capture square, promotion) we need.
    let mut flags: u8 = 0;
    let mut captured: u8 = if target != EMPTY {
        flags |= move_flags::CAPTURE;
        target
    } else {
        0
    };
    let mut ep_capture_sq: Option<u8> = None;
    let mut new_ep: Option<u8> = None;
    let mut castle_rook: Option<(u8, u8)> = None; // (from, to)
    let mover_kind = kind(piece);

    match mover_kind {
        1 => validate_pawn(state, from, to, mv.promotion, &mut flags, &mut ep_capture_sq, &mut new_ep)?,
        2 => validate_knight(from, to)?,
        3 => validate_slider(state, from, to, &[7i8, 9, -7, -9])?,
        4 => validate_slider(state, from, to, &[1i8, -1, 8, -8])?,
        5 => validate_slider(state, from, to, &[1i8, -1, 8, -8, 7, 9, -7, -9])?,
        6 => validate_king(state, from, to, &mut flags, &mut castle_rook)?,
        _ => return Err(MoveError::IllegalShape),
    }

    // Promotion sanity: pawn-only, only on last rank, must specify a real piece.
    if mover_kind == 1 {
        let last_rank = if state.side_to_move == 0 { 7 } else { 0 };
        if rank_of(to) == last_rank {
            match mv.promotion {
                2 | 3 | 4 | 5 => {}
                _ => return Err(MoveError::PromotionRequired),
            }
            flags |= move_flags::PROMOTION;
        } else if mv.promotion != 0 {
            return Err(MoveError::InvalidPromotion);
        }
    } else if mv.promotion != 0 {
        return Err(MoveError::InvalidPromotion);
    }

    // Snapshot for rollback if move leaves king in check.
    let snapshot = state.clone();

    // --- mutate -------------------------------------------------------
    // 1. Move the piece (with promotion if applicable).
    let placed_piece = if (flags & move_flags::PROMOTION) != 0 {
        make(mv.promotion, state.side_to_move)
    } else {
        piece
    };
    state.set(from, EMPTY);
    state.set(to, placed_piece);

    // 2. En-passant capture removes the captured pawn from its actual square,
    //    not the destination.
    if let Some(epsq) = ep_capture_sq {
        captured = state.at(epsq);
        flags |= move_flags::CAPTURE | move_flags::EN_PASSANT;
        state.set(epsq, EMPTY);
    }

    // 3. Castling rook hop.
    if let Some((rfrom, rto)) = castle_rook {
        let rook = state.at(rfrom);
        state.set(rfrom, EMPTY);
        state.set(rto, rook);
    }

    // 4. Update castling rights — any king/rook movement OR rook capture
    //    permanently revokes the corresponding right.
    update_castling_rights(state, from, to);

    // 5. King-safety check — if the move leaves our own king attacked,
    //    roll back and reject.
    let our_king = state.king_square(state.side_to_move).ok_or(MoveError::IllegalShape)?;
    if is_square_attacked(state, our_king, 1 - state.side_to_move) {
        *state = snapshot;
        return Err(MoveError::KingLeftInCheck);
    }

    // 6. Halfmove clock: reset on capture or pawn push, else increment.
    if mover_kind == 1 || (flags & move_flags::CAPTURE) != 0 {
        state.halfmove_clock = 0;
    } else {
        state.halfmove_clock = state.halfmove_clock.saturating_add(1);
    }

    // 7. Fullmove counter increments after black's move.
    if state.side_to_move == 1 {
        state.fullmove_number = state.fullmove_number.saturating_add(1);
    }

    // 8. En-passant target reset, then set if this move was a double pawn push.
    state.en_passant = new_ep;

    // 9. Side flip BEFORE check/mate detection so we test the *responder*'s king.
    state.side_to_move ^= 1;

    let opp_king = state.king_square(state.side_to_move).ok_or(MoveError::IllegalShape)?;
    let opp_in_check = is_square_attacked(state, opp_king, 1 - state.side_to_move);
    let any_legal = side_to_move_has_any_legal_move(state);

    let mut is_mate = false;
    let mut is_stalemate = false;
    if !any_legal {
        if opp_in_check {
            flags |= move_flags::MATE | move_flags::CHECK;
            is_mate = true;
        } else {
            is_stalemate = true;
        }
    } else if opp_in_check {
        flags |= move_flags::CHECK;
    }

    let draw_50_move = state.halfmove_clock >= 100;
    let draw_insufficient = is_insufficient_material(state);

    Ok(MoveOutcome {
        piece,
        captured,
        flags,
        is_check: opp_in_check,
        is_mate,
        is_stalemate,
        draw_50_move,
        draw_insufficient,
    })
}

/// Recompute terminal status for a `state` whose side-to-move is the
/// player on the clock. Used by the on-chain `make_move` handler after
/// `apply_move` returns, and also when timing out / claiming draws.
pub fn compute_status(state: &ChessState) -> Status {
    let king = match state.king_square(state.side_to_move) {
        Some(k) => k,
        None => return Status::Ongoing,
    };
    let in_check = is_square_attacked(state, king, 1 - state.side_to_move);
    let any_legal = side_to_move_has_any_legal_move(state);

    if !any_legal {
        if in_check {
            return Status::Checkmate {
                winner: 1 - state.side_to_move,
            };
        }
        return Status::Stalemate;
    }
    if state.halfmove_clock >= 100 {
        return Status::FiftyMove;
    }
    if is_insufficient_material(state) {
        return Status::InsufficientMaterial;
    }
    Status::Ongoing
}

// =====================================================================
// Per-piece validators
// =====================================================================

fn validate_pawn(
    state: &ChessState,
    from: u8,
    to: u8,
    promotion: u8,
    flags: &mut u8,
    ep_capture_sq: &mut Option<u8>,
    new_ep: &mut Option<u8>,
) -> Result<(), MoveError> {
    let _ = promotion; // promotion validated by caller
    let dir: i8 = if state.side_to_move == 0 { 8 } else { -8 };
    let start_rank: u8 = if state.side_to_move == 0 { 1 } else { 6 };
    let from_i = from as i8;
    let to_i = to as i8;
    let delta = to_i - from_i;

    let target = state.at(to);

    // Single push.
    if delta == dir {
        if target != EMPTY {
            return Err(MoveError::IllegalShape);
        }
        return Ok(());
    }
    // Double push.
    if delta == 2 * dir && rank_of(from) == start_rank {
        let intermediate = (from_i + dir) as u8;
        if state.at(intermediate) != EMPTY || target != EMPTY {
            return Err(MoveError::PathBlocked);
        }
        *new_ep = Some(intermediate);
        *flags |= move_flags::DOUBLE_PUSH;
        return Ok(());
    }
    // Captures (incl. en passant). Must change file by exactly 1.
    let file_delta = (file_of(to) as i8) - (file_of(from) as i8);
    if (delta == dir + 1 || delta == dir - 1) && file_delta.abs() == 1 {
        if target != EMPTY {
            return Ok(());
        }
        // En passant: target must match the live ep square.
        if let Some(ep) = state.en_passant {
            if ep == to {
                let cap_sq = if state.side_to_move == 0 { to - 8 } else { to + 8 };
                let cap = state.at(cap_sq);
                if cap == EMPTY || color(cap) == state.side_to_move {
                    return Err(MoveError::IllegalEnPassant);
                }
                *ep_capture_sq = Some(cap_sq);
                return Ok(());
            }
        }
        return Err(MoveError::IllegalShape);
    }

    Err(MoveError::IllegalShape)
}

fn validate_knight(from: u8, to: u8) -> Result<(), MoveError> {
    let df = (file_of(to) as i8 - file_of(from) as i8).abs();
    let dr = (rank_of(to) as i8 - rank_of(from) as i8).abs();
    if (df == 1 && dr == 2) || (df == 2 && dr == 1) {
        Ok(())
    } else {
        Err(MoveError::IllegalShape)
    }
}

/// Validates a sliding piece (B/R/Q) by walking the ray from `from` to
/// `to`, ensuring all intermediate squares are empty and the offset
/// matches one of the legal directions.
fn validate_slider(state: &ChessState, from: u8, to: u8, dirs: &[i8]) -> Result<(), MoveError> {
    let from_i = from as i8;
    let to_i = to as i8;

    for &d in dirs {
        let mut cur = from_i + d;
        while (0..64).contains(&cur) && !crosses_edge(cur - d, cur) {
            if cur == to_i {
                return Ok(());
            }
            if state.squares[cur as usize] != EMPTY {
                if cur == to_i {
                    return Ok(());
                }
                break;
            }
            cur += d;
        }
    }
    Err(MoveError::IllegalShape)
}

/// File-wrap detector. When walking a ray by ±1, ±7, ±9 we can wrap
/// around the board edge (e.g. h-file -> a-file). We detect this by
/// checking that the file delta between successive squares is at most 1.
#[inline(always)]
fn crosses_edge(prev: i8, cur: i8) -> bool {
    let pf = (prev & 7) as i8;
    let cf = (cur & 7) as i8;
    (pf - cf).abs() > 1
}

fn validate_king(
    state: &ChessState,
    from: u8,
    to: u8,
    flags: &mut u8,
    castle_rook: &mut Option<(u8, u8)>,
) -> Result<(), MoveError> {
    let df = (file_of(to) as i8 - file_of(from) as i8).abs();
    let dr = (rank_of(to) as i8 - rank_of(from) as i8).abs();

    // Standard one-square king move.
    if df <= 1 && dr <= 1 {
        return Ok(());
    }

    // Castling: same rank, two-square king hop, on the home rank, with rights.
    if dr == 0 && df == 2 {
        let home_rank = if state.side_to_move == 0 { 0 } else { 7 };
        if rank_of(from) != home_rank || rank_of(to) != home_rank {
            return Err(MoveError::IllegalCastle);
        }
        let king_side = file_of(to) == 6;
        let rights_bit = match (state.side_to_move, king_side) {
            (0, true) => castle::W_KING_SIDE,
            (0, false) => castle::W_QUEEN_SIDE,
            (1, true) => castle::B_KING_SIDE,
            (1, false) => castle::B_QUEEN_SIDE,
            _ => 0,
        };
        if state.castling_rights & rights_bit == 0 {
            return Err(MoveError::IllegalCastle);
        }

        // Rook home / target squares + path check.
        let (rook_from, rook_to, path_squares): (u8, u8, &[u8]) = if king_side {
            // King e->g, Rook h->f, path squares f & g must be empty
            (sq(7, home_rank), sq(5, home_rank), &[5, 6])
        } else {
            // King e->c, Rook a->d, path b/c/d must be empty (but b not crossed by king)
            (sq(0, home_rank), sq(3, home_rank), &[1, 2, 3])
        };
        for &f in path_squares {
            if state.at(sq(f, home_rank)) != EMPTY {
                return Err(MoveError::IllegalCastle);
            }
        }
        // Verify the rook is actually home and right colour.
        let expected_rook = if state.side_to_move == 0 { W_ROOK } else { B_ROOK };
        if state.at(rook_from) != expected_rook {
            return Err(MoveError::IllegalCastle);
        }
        // King must NOT be in check, must NOT cross an attacked square,
        // and must NOT land on an attacked square (latter handled by the
        // post-move king-safety check, but we test traversal here).
        let king_path: &[u8] = if king_side {
            &[sq(4, home_rank), sq(5, home_rank), sq(6, home_rank)]
        } else {
            &[sq(4, home_rank), sq(3, home_rank), sq(2, home_rank)]
        };
        for &k in king_path {
            if is_square_attacked(state, k, 1 - state.side_to_move) {
                return Err(MoveError::IllegalCastle);
            }
        }

        *flags |= move_flags::CASTLE;
        *castle_rook = Some((rook_from, rook_to));
        return Ok(());
    }

    Err(MoveError::IllegalShape)
}

// =====================================================================
// Castling-rights bookkeeping
// =====================================================================

fn update_castling_rights(state: &mut ChessState, from: u8, to: u8) {
    // King moves wipe both rights for that colour.
    if from == 4 {
        state.castling_rights &= !(castle::W_KING_SIDE | castle::W_QUEEN_SIDE);
    }
    if from == 60 {
        state.castling_rights &= !(castle::B_KING_SIDE | castle::B_QUEEN_SIDE);
    }
    // Rook leaves home OR is captured on home.
    let touched = [from, to];
    for &t in &touched {
        match t {
            0 => state.castling_rights &= !castle::W_QUEEN_SIDE,
            7 => state.castling_rights &= !castle::W_KING_SIDE,
            56 => state.castling_rights &= !castle::B_QUEEN_SIDE,
            63 => state.castling_rights &= !castle::B_KING_SIDE,
            _ => {}
        }
    }
}

// =====================================================================
// Attack detection — answers "is square `sq` attacked by `by_color`?"
// =====================================================================

pub fn is_square_attacked(state: &ChessState, target: u8, by_color: u8) -> bool {
    // Pawn attacks: black pawns attack diagonally down, white pawns up.
    let pawn = if by_color == 0 { W_PAWN } else { B_PAWN };
    let pawn_dirs: [i8; 2] = if by_color == 0 { [-7, -9] } else { [7, 9] };
    // ^ from the target's POV, a white-pawn attacker sits one rank below.
    for &d in &pawn_dirs {
        let s = target as i8 + d;
        if (0..64).contains(&s) && !crosses_edge(target as i8, s) {
            if state.squares[s as usize] == pawn {
                return true;
            }
        }
    }

    // Knight attacks
    let knight = if by_color == 0 { W_KNIGHT } else { B_KNIGHT };
    const KNIGHT_DELTAS: [(i8, i8); 8] = [
        (1, 2), (2, 1), (2, -1), (1, -2),
        (-1, -2), (-2, -1), (-2, 1), (-1, 2),
    ];
    let tf = file_of(target) as i8;
    let tr = rank_of(target) as i8;
    for (df, dr) in KNIGHT_DELTAS {
        let nf = tf + df;
        let nr = tr + dr;
        if (0..8).contains(&nf) && (0..8).contains(&nr) {
            if state.at(sq(nf as u8, nr as u8)) == knight {
                return true;
            }
        }
    }

    // Bishop/Queen diagonals
    let bishop = if by_color == 0 { W_BISHOP } else { B_BISHOP };
    let queen = if by_color == 0 { W_QUEEN } else { B_QUEEN };
    let rook = if by_color == 0 { W_ROOK } else { B_ROOK };
    let king = if by_color == 0 { W_KING } else { B_KING };

    if scan_ray_for(state, target, &[7, 9, -7, -9], &[bishop, queen]) {
        return true;
    }
    if scan_ray_for(state, target, &[1, -1, 8, -8], &[rook, queen]) {
        return true;
    }

    // King adjacency
    for df in -1i8..=1 {
        for dr in -1i8..=1 {
            if df == 0 && dr == 0 {
                continue;
            }
            let nf = tf + df;
            let nr = tr + dr;
            if (0..8).contains(&nf) && (0..8).contains(&nr) {
                if state.at(sq(nf as u8, nr as u8)) == king {
                    return true;
                }
            }
        }
    }

    false
}

fn scan_ray_for(state: &ChessState, from: u8, dirs: &[i8], pieces: &[u8]) -> bool {
    for &d in dirs {
        let mut cur = from as i8 + d;
        let mut prev = from as i8;
        while (0..64).contains(&cur) && !crosses_edge(prev, cur) {
            let p = state.squares[cur as usize];
            if p != EMPTY {
                if pieces.contains(&p) {
                    return true;
                }
                break;
            }
            prev = cur;
            cur += d;
        }
    }
    false
}

// =====================================================================
// Legality probe — does the side to move have ANY legal move?
// Used for mate/stalemate detection. We early-exit on the first legal
// move found, so worst-case is bounded by board size.
// =====================================================================

pub fn side_to_move_has_any_legal_move(state: &ChessState) -> bool {
    for from in 0..64u8 {
        let p = state.at(from);
        if p == EMPTY || color(p) != state.side_to_move {
            continue;
        }
        for to in 0..64u8 {
            if from == to {
                continue;
            }
            // Iterate possible promotions only on last rank (else 0).
            let promos: &[u8] = if kind(p) == 1
                && ((state.side_to_move == 0 && rank_of(to) == 7)
                    || (state.side_to_move == 1 && rank_of(to) == 0))
            {
                &[5, 4, 3, 2]
            } else {
                &[0]
            };
            for &promo in promos {
                let mut probe = state.clone();
                if apply_move(&mut probe, ChessMove { from, to, promotion: promo }).is_ok() {
                    return true;
                }
            }
        }
    }
    false
}

// =====================================================================
// Insufficient material — K vs K, K+B vs K, K+N vs K, K+B vs K+B same colour.
// Anything beyond that we leave to the 50-move rule / draw agreement.
// =====================================================================

pub fn is_insufficient_material(state: &ChessState) -> bool {
    let mut wp = [0u8; 7]; // [_, P, N, B, R, Q, K]
    let mut bp = [0u8; 7];
    let mut white_bishop_squares = Vec::with_capacity(2);
    let mut black_bishop_squares = Vec::with_capacity(2);
    for i in 0..64u8 {
        let p = state.at(i);
        if p == EMPTY {
            continue;
        }
        let k = kind(p);
        if is_white(p) {
            wp[k as usize] += 1;
            if k == 3 {
                white_bishop_squares.push(i);
            }
        } else {
            bp[k as usize] += 1;
            if k == 3 {
                black_bishop_squares.push(i);
            }
        }
    }
    // Any pawn / rook / queen disqualifies trivial-draw detection.
    if wp[1] + bp[1] + wp[4] + bp[4] + wp[5] + bp[5] > 0 {
        return false;
    }
    let wn = wp[2];
    let bn = bp[2];
    let wb = wp[3];
    let bb = bp[3];

    // K vs K
    if wn + bn + wb + bb == 0 {
        return true;
    }
    // K+minor vs K
    if wn + wb == 1 && bn + bb == 0 {
        return true;
    }
    if bn + bb == 1 && wn + wb == 0 {
        return true;
    }
    // K+B vs K+B with bishops on same colour
    if wb == 1 && bb == 1 && wn == 0 && bn == 0 {
        let wsq = white_bishop_squares[0];
        let bsq = black_bishop_squares[0];
        let wcol = (file_of(wsq) + rank_of(wsq)) & 1;
        let bcol = (file_of(bsq) + rank_of(bsq)) & 1;
        if wcol == bcol {
            return true;
        }
    }
    false
}
