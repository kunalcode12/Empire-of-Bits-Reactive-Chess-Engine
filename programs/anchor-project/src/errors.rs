// =====================================================================
// errors.rs
// ---------------------------------------------------------------------
// Centralised, exhaustive error enum for the Reactive Chess protocol.
// Every illegal state transition or constraint violation routes through
// here so client SDKs, indexers and overlays can map error codes to
// user-facing strings or telemetry buckets.
//
// Numbering is stable: do NOT renumber existing variants — third-party
// indexers (Helius, Geyser consumers, Discord/Twitch bots) parse the
// `code` field and rely on it being immutable for the lifetime of the
// deployed program.
// =====================================================================

use anchor_lang::prelude::*;

#[error_code]
pub enum ChessError {
    // --- Game lifecycle -------------------------------------------------
    #[msg("Game is not in the expected state for this instruction")]
    InvalidGameStatus,
    #[msg("Game is already full — both colours occupied")]
    GameAlreadyFull,
    #[msg("Game has not yet started")]
    GameNotStarted,
    #[msg("Game already has a result; close it instead")]
    GameAlreadyOver,
    #[msg("Cannot join your own game on both sides")]
    SelfPlayForbidden,

    // --- Authority / signer checks --------------------------------------
    #[msg("Signer is not a participating player")]
    NotAPlayer,
    #[msg("Signer is the wrong colour for this action")]
    WrongColor,
    #[msg("It is not this player's turn")]
    NotYourTurn,
    #[msg("Only the game creator may close an unstarted game")]
    NotCreator,

    // --- Move validation -----------------------------------------------
    #[msg("Source square is empty")]
    EmptyFromSquare,
    #[msg("Source piece does not belong to the moving player")]
    WrongPieceColor,
    #[msg("Move geometry is illegal for the moving piece")]
    IllegalMoveShape,
    #[msg("Path between from/to is blocked")]
    PathBlocked,
    #[msg("Destination square is occupied by a friendly piece")]
    FriendlyCapture,
    #[msg("Move would leave own king in check")]
    KingLeftInCheck,
    #[msg("Promotion piece is required but missing or invalid")]
    PromotionRequired,
    #[msg("Promotion was supplied for a non-promoting move")]
    InvalidPromotion,
    #[msg("Castling is not legal in this position")]
    IllegalCastle,
    #[msg("En passant capture is not legal in this position")]
    IllegalEnPassant,
    #[msg("Move history overflowed the on-chain ring buffer")]
    MoveHistoryOverflow,
    #[msg("Submitted move nonce does not match the current ply — possible replay or race")]
    StaleMoveNonce,

    // --- Draws ----------------------------------------------------------
    #[msg("No draw offer is currently outstanding")]
    NoDrawOffer,
    #[msg("Draw offer is from the wrong colour")]
    DrawOfferFromWrongColor,
    #[msg("Cannot accept your own draw offer")]
    CannotAcceptOwnDrawOffer,

    // --- Clocks ---------------------------------------------------------
    #[msg("Clock has already flagged for the moving side")]
    ClockFlagged,
    #[msg("Clock sync called too early — minimum interval not elapsed")]
    ClockSyncTooSoon,

    // --- Stream / reactions --------------------------------------------
    #[msg("Stream session metadata exceeds maximum length")]
    StreamMetaTooLong,
    #[msg("Reaction payload exceeds maximum length")]
    ReactionTooLong,
    #[msg("Comment exceeds maximum length")]
    CommentTooLong,

    // --- Wager / vault --------------------------------------------------
    #[msg("Wager amounts must match between players")]
    WagerMismatch,
    #[msg("Wager vault has insufficient balance")]
    WagerVaultUnderfunded,

    // --- Misc -----------------------------------------------------------
    #[msg("Numerical overflow")]
    MathOverflow,
    #[msg("Account size mismatch — IDL/program drift")]
    AccountSizeMismatch,
    #[msg("Time-control configuration is invalid")]
    InvalidTimeControl,
}
