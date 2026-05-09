# Reactive Multiplayer Chess Engine — Solana / Anchor

A production-grade, livestream-native chess protocol on Solana. Designed
to feel like **"Twitch Plays Chess meets modern esports infrastructure"** —
every move, check, mate, draw and audience reaction is a structured
on-chain event that overlays, AI commentators, Twitch/Discord/YouTube
bots and analytics engines can subscribe to in real time.

This is **infrastructure**, not a chess app. Frontends, indexers and
bots all consume the same event stream. The on-chain program is the
source of truth for chess legality, clocks, wagers and game outcomes.

```
            ┌──────────────┐         ┌──────────────────┐
   wallet ──│  Solana RPC  │── tx ──▶│  Chess Program   │
            └──────────────┘         │  (this repo)     │
                   ▲                 └────────┬─────────┘
                   │                          │ events
                   │                          ▼
            ┌──────────────┐         ┌──────────────────┐
            │  onLogs +    │◀────────│ MovePlayed,      │
            │  Helius ws + │         │ CheckmateEvent,  │
            │  Geyser/grpc │         │ StreamReaction…  │
            └──────┬───────┘         └──────────────────┘
                   │
            ┌──────┴────────┬────────────┬──────────┐
            ▼               ▼            ▼          ▼
        Overlays       AI commentator   Bots    Analytics
```

---

## Features

- **Full on-chain chess legality** — pseudo-legal generation per piece
  + king-safety filter; castling, en passant, promotion, mate &
  stalemate detection, 50-move rule, insufficient-material draws.
- **Multiple concurrent matches per creator** via PDA seeded by
  `(creator, session_id)`.
- **PDA-only design** — no extra signer keypairs anywhere; clean for
  Solana Mobile Stack wallet adapters.
- **Reactive event layer** — every state change emits a structured
  Anchor event with stable field names and a `board_hash` for
  client-side replay verification.
- **Fischer clocks** with on-chain timeout flagging callable by anyone.
- **Wager escrow** with automatic payout on mate/timeout/resignation
  and 50/50 split on agreed/forced draws.
- **Anti-replay nonce** so retried txs from a flaky network can't apply
  the same move twice.
- **Compressed move history** — 16-bit packed plies; a full game fits
  in ~512 bytes of account data.
- **Spectator/reaction surface** with emote, prediction, comment and
  clip-marker payloads — emitted as events so reactions don't bloat
  the on-chain account.

---

## Layout

```
programs/anchor-project/src/
├── lib.rs               # #[program] entrypoint — wires up handlers
├── errors.rs            # stable, numbered error codes
├── events.rs            # all Anchor #[event] structs (the reactive layer)
├── state.rs             # Game / StreamSession / WagerVault accounts
├── chess/
│   ├── mod.rs           # piece encoding, castling/move-flag bitfields
│   ├── board.rs         # ChessState + ChessMove + Zobrist-style hash
│   └── movegen.rs       # apply_move, status, attack detection
└── instructions/
    ├── mod.rs           # finalize_game + settle_wager helpers
    ├── create_game.rs
    ├── join_game.rs
    ├── make_move.rs     # the hot path
    ├── resign_game.rs
    ├── offer_draw.rs
    ├── accept_draw.rs
    ├── sync_clock.rs    # callable by anyone — flag a timed-out player
    ├── record_reaction.rs
    └── close_game.rs

app/
├── client.ts            # ChessClient SDK + runnable demo
└── subscribe.ts         # three websocket subscription patterns

tests/
└── anchor-project.ts    # end-to-end Anchor mocha suite
```

---

## Quick start

```bash
yarn install          # already done in this repo
anchor build          # compiles the program + IDL + TS types
anchor test           # spins up local validator and runs the suite
```

`anchor test` boots `solana-test-validator`, deploys the program at
the id baked into [Anchor.toml](Anchor.toml), funds keypairs, and runs
the suite in [tests/anchor-project.ts](tests/anchor-project.ts).

To run the standalone client demo against the local validator:

```bash
solana-test-validator -r &                            # in another shell
ANCHOR_PROVIDER_URL=http://127.0.0.1:8899 \
ANCHOR_WALLET=$HOME/.config/solana/id.json \
  npx ts-node app/client.ts
```

To watch the event stream in real time:

```bash
ANCHOR_PROVIDER_URL=http://127.0.0.1:8899 \
  npx ts-node app/subscribe.ts
```

---

## Instructions

Each instruction is a single file under [programs/anchor-project/src/instructions/](programs/anchor-project/src/instructions/).

| Instruction       | Signer       | Purpose                                                                 |
|-------------------|--------------|-------------------------------------------------------------------------|
| `create_game`     | creator      | Mint a `Game` PDA. Creator becomes white. Optionally escrows a wager.   |
| `join_game`       | black        | Seat black; matches the wager; flips status → `Active`.                 |
| `make_move`       | mover        | Validate via the chess engine, tick clock, emit events, finalise on mate. |
| `resign_game`     | either       | Opponent wins; wager goes to opponent.                                   |
| `offer_draw`      | either       | Set `draw_offer`. Auto-cancels on next `make_move`.                     |
| `accept_draw`     | counterparty | Finalise as draw; split wager 50/50.                                    |
| `sync_clock`      | anyone       | Tick the on-chain clock; flag a timed-out player.                       |
| `record_reaction` | spectator    | Emit `StreamReactionEvent`; bump spectator/reaction counters.           |
| `close_game`      | creator/player | Reclaim rent on a finished (or never-started) match.                  |

---

## Events (the reactive layer)

Every event is a flat `#[event]` carrying `game: Pubkey`, the current
`ply`, and a unix timestamp so consumers can correlate without a
second RPC round-trip.

| Event                  | Emitted on                                                  |
|------------------------|-------------------------------------------------------------|
| `GameCreated`          | `create_game`                                               |
| `GameStarted`          | `join_game`                                                 |
| `MovePlayed`           | every successful `make_move`                                |
| `CheckEvent`           | move that gives check (without mate)                        |
| `CheckmateEvent`       | mate                                                        |
| `StalemateEvent`       | stalemate                                                   |
| `DrawEvent`            | agreed / 50-move / insufficient material / stalemate        |
| `ClockUpdated`         | `sync_clock` (no flag-fall)                                 |
| `DrawOffered`          | `offer_draw`                                                |
| `StreamReactionEvent`  | `record_reaction` — emote / prediction / comment / clip     |
| `SpectatorJoined`      | first reaction with `register_as_spectator: true`           |
| `GameEnded`            | terminal state (mate / resign / timeout / draw / abandoned) |

`MovePlayed.flags` is a packed bitfield: `CAPTURE | CASTLE | EN_PASSANT
| CHECK | MATE | DOUBLE_PUSH | PROMOTION` — see [chess/mod.rs](programs/anchor-project/src/chess/mod.rs).

`MovePlayed.board_hash` is an FNV-1a hash of the position; clients can
maintain their own board state and assert against this on every move
to detect any drift.

---

## Subscribing

Three patterns are demonstrated in [app/subscribe.ts](app/subscribe.ts),
ordered by infrastructure / latency tradeoff:

1. **Public RPC `onLogs` + `BorshCoder`** — works on any Solana RPC,
   zero infra. Latency = cluster confirm time.
2. **Helius enhanced websocket** (`transactionSubscribe` filtered to
   `programIds`) — sub-second push delivery, structured payloads, no
   parsing overhead.
3. **Geyser plug-in → Kafka/Redpanda** — for high-throughput analytics
   pipelines. Yellowstone gRPC is the recommended source.

Pattern 1 is the runnable demo; (2) and (3) are scaffolded with
connection details factored out so production deployments swap just
the URL.

---

## PDA seeds

```
game     = ["game",   creator,    session_id_le_bytes]
stream   = ["stream", game_pubkey]
vault    = ["vault",  game_pubkey]   # system-owned, lamport-only
```

PDA helpers live at [app/client.ts:33](app/client.ts#L33).

---

## Account size budget

`Game` (≈940 bytes after Anchor disc) holds the full board, both
clocks, draw offer, wager metadata, packed move history (256 plies)
and stream metadata (128 bytes). The packed history is bounded by
`MAX_MOVE_HISTORY` in [state.rs:23](programs/anchor-project/src/state.rs#L23);
on overflow the game finalises as `Abandoned` rather than corrupting
state.

`StreamSession` is split out so high-frequency reactions don't fragment
the hot Game account and so it can be closed independently.

---

## Security model

- **Authority**: every player-only ix checks signer vs `game.white` /
  `game.black`; spectators can only call `record_reaction`.
- **Turn enforcement**: `expected_color == game.chess.side_to_move`,
  rejected with `NotYourTurn`.
- **Replay protection**: monotonic `move_nonce` echoed by the client;
  mismatch → `StaleMoveNonce`.
- **Race conditions**: account writes are sequenced by Solana itself;
  the nonce closes the window where a retry could apply twice.
- **Legality**: enforced entirely on-chain by [chess/movegen.rs](programs/anchor-project/src/chess/movegen.rs)
  — clients cannot push illegal state.
- **Wager safety**: vault is a system-owned PDA; lamport math goes
  through `try_borrow_mut_lamports` with checked arithmetic.

---

## Extension points

These are designed-in but left for follow-up work:

- **Compressed replay storage** — `move_history: Vec<u16>` is already
  pack-friendly; a separate `Replay` PDA can persist long matches.
- **Memory NFTs** — finalised `Game` accounts are addressable post-game
  and ready for a "mint replay NFT" CPI driven by the move history.
- **Tournaments** — `session_id` is part of the PDA seed so a
  dispatcher can pre-derive bracket matches deterministically.
- **AI commentator hooks** — `MakeMoveArgs.comment` (per-move
  narration) + `StreamReactionEvent { kind: 2 }` (free-form text).
- **Audience predictions** — `StreamReactionEvent { kind: 1, payload }`
  carries 32 bytes of structured prediction metadata.
- **Solana Mobile Stack** — PDA-only design means SMS wallet adapters
  work without any additional signers.
- **Helius / Geyser webhooks** — events are flat with explicit `game`,
  `ply`, `at` so no enrichment RPCs are needed downstream.

---

## Compute & cost notes

- A typical `make_move` runs in **~25-40k CU** on BPF including legality
  + status detection, well inside the 200k default.
- Storage cost for one `Game` + `StreamSession` is approximately the
  rent-exempt minimum for ~1.1 KB total — refunded on `close_game`.

---

## License

ISC — see [package.json](package.json).
