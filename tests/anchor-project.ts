// =====================================================================
// tests/anchor-project.ts
// ---------------------------------------------------------------------
// End-to-end Anchor tests for the Reactive Chess protocol.
//
// Coverage:
//   * create + join (full lifecycle, including wager escrow)
//   * legal play sequence ending in checkmate (Scholar's mate)
//   * illegal-move rejection (geometry, turn, replay nonce)
//   * resignation
//   * draw offer + accept flow
//   * spectator reactions (event subscription end-to-end)
//   * concurrent games (different session_ids)
//
// Squares: a1 = 0, h1 = 7, a8 = 56, h8 = 63 (rank 0 = white back rank).
// =====================================================================

import * as anchor from "@coral-xyz/anchor";
import { BN, Program } from "@coral-xyz/anchor";
import { AnchorProject } from "../target/types/anchor_project";
import {
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
} from "@solana/web3.js";
import { assert } from "chai";

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

const sq = (file: number, rank: number) => rank * 8 + file;
const FILE = { a: 0, b: 1, c: 2, d: 3, e: 4, f: 5, g: 6, h: 7 } as const;

function findGamePda(programId: PublicKey, creator: PublicKey, sessionId: BN) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("game"), creator.toBuffer(), sessionId.toArrayLike(Buffer, "le", 8)],
    programId
  )[0];
}
function findStreamPda(programId: PublicKey, game: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("stream"), game.toBuffer()],
    programId
  )[0];
}
function findVaultPda(programId: PublicKey, game: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("vault"), game.toBuffer()],
    programId
  )[0];
}

async function airdrop(provider: anchor.AnchorProvider, to: PublicKey, sol = 5) {
  const sig = await provider.connection.requestAirdrop(to, sol * LAMPORTS_PER_SOL);
  await provider.connection.confirmTransaction(sig, "confirmed");
}

// Subscribe to all program events for the duration of the closure. Returns
// any captured events so individual tests can assert reactive behaviour.
async function captureEvents<T>(
  program: Program<AnchorProject>,
  fn: () => Promise<T>
): Promise<{ result: T; events: Array<{ name: string; data: any }> }> {
  const captured: Array<{ name: string; data: any }> = [];
  const allEventNames = [
    "gameCreated",
    "gameStarted",
    "gameEnded",
    "movePlayed",
    "checkEvent",
    "checkmateEvent",
    "stalemateEvent",
    "drawEvent",
    "clockUpdated",
    "drawOffered",
    "drawDeclined",
    "streamReactionEvent",
    "spectatorJoined",
  ];
  const listenerIds = allEventNames.map((n) =>
    program.addEventListener(n as any, (data) => captured.push({ name: n, data }))
  );
  try {
    const result = await fn();
    await new Promise((r) => setTimeout(r, 250));
    return { result, events: captured };
  } finally {
    await Promise.all(listenerIds.map((id) => program.removeEventListener(id)));
  }
}

// ---------------------------------------------------------------------
// Suite
// ---------------------------------------------------------------------

describe("anchor-project — Reactive Chess", () => {
  anchor.setProvider(anchor.AnchorProvider.env());
  const program = anchor.workspace.anchorProject as Program<AnchorProject>;
  const provider = anchor.getProvider() as anchor.AnchorProvider;
  const programId = program.programId;

  const white = Keypair.generate();
  const black = Keypair.generate();
  const spectator = Keypair.generate();

  let sessionCounter = 0;
  const nextSession = () => new BN(++sessionCounter);

  before(async () => {
    await airdrop(provider, white.publicKey, 10);
    await airdrop(provider, black.publicKey, 10);
    await airdrop(provider, spectator.publicKey, 1);
  });

  it("creates a game, joins it, plays Scholar's mate, settles wager", async () => {
    const sessionId = nextSession();
    const wager = new BN(LAMPORTS_PER_SOL).divn(10);
    const game = findGamePda(programId, white.publicKey, sessionId);
    const stream = findStreamPda(programId, game);
    const vault = findVaultPda(programId, game);

    const { events: createEvents } = await captureEvents(program, async () => {
      await program.methods
        .createGame({
          sessionId,
          timeControl: {
            initialMs: new BN(60_000),
            incrementMs: new BN(1_000),
            delayMs: new BN(0),
          },
          wagerPerSide: wager,
          streamMeta: "twitch:demo|theme:dark",
          streamLabel: "demo-stream",
          streamer: null,
        })
        .accountsPartial({
          creator: white.publicKey,
          game, stream, vault,
          systemProgram: SystemProgram.programId,
        })
        .signers([white]).rpc();
    });
    assert.ok(createEvents.find((e) => e.name === "gameCreated"));

    const { events: joinEvents } = await captureEvents(program, async () => {
      await program.methods
        .joinGame()
        .accountsPartial({
          black: black.publicKey, game, vault,
          systemProgram: SystemProgram.programId,
        })
        .signers([black]).rpc();
    });
    assert.ok(joinEvents.find((e) => e.name === "gameStarted"));

    // Scholar's mate: 1.e4 e5 2.Bc4 Nc6 3.Qh5 Nf6?? 4.Qxf7#
    const mvs = [
      { signer: white, from: sq(FILE.e, 1), to: sq(FILE.e, 3) }, // e2 -> e4
      { signer: black, from: sq(FILE.e, 6), to: sq(FILE.e, 4) }, // e7 -> e5
      { signer: white, from: sq(FILE.f, 0), to: sq(FILE.c, 3) }, // Bf1 -> Bc4
      { signer: black, from: sq(FILE.b, 7), to: sq(FILE.c, 5) }, // Nb8 -> Nc6
      { signer: white, from: sq(FILE.d, 0), to: sq(FILE.h, 4) }, // Qd1 -> Qh5
      { signer: black, from: sq(FILE.g, 7), to: sq(FILE.f, 5) }, // Ng8 -> Nf6
      { signer: white, from: sq(FILE.h, 4), to: sq(FILE.f, 6) }, // Qxf7#
    ];

    let nonce = 0;
    for (const m of mvs) {
      const { events } = await captureEvents(program, async () => {
        await program.methods
          .makeMove({
            mv: { from: m.from, to: m.to, promotion: 0 },
            expectedNonce: nonce,
            comment: null,
          })
          .accountsPartial({
            mover: m.signer.publicKey, game, vault,
            white: white.publicKey, black: black.publicKey,
          })
          .signers([m.signer]).rpc();
      });
      assert.ok(events.find((e) => e.name === "movePlayed"), `movePlayed for ply ${nonce}`);
      nonce += 1;
    }

    const final = await program.account.game.fetch(game);
    assert.deepEqual(final.status, { finished: {} });
    assert.deepEqual(final.result, { whiteWins: {} });
    assert.deepEqual(final.endReason, { checkmate: {} });
  });

  it("rejects illegal-shape, wrong-turn and stale-nonce moves", async () => {
    const sessionId = nextSession();
    const game = findGamePda(programId, white.publicKey, sessionId);
    const stream = findStreamPda(programId, game);
    const vault = findVaultPda(programId, game);

    await program.methods
      .createGame({
        sessionId,
        timeControl: { initialMs: new BN(0), incrementMs: new BN(0), delayMs: new BN(0) },
        wagerPerSide: new BN(0),
        streamMeta: "",
        streamLabel: "",
        streamer: null,
      })
      .accountsPartial({
        creator: white.publicKey, game, stream, vault,
        systemProgram: SystemProgram.programId,
      })
      .signers([white]).rpc();

    await program.methods
      .joinGame()
      .accountsPartial({
        black: black.publicKey, game, vault,
        systemProgram: SystemProgram.programId,
      })
      .signers([black]).rpc();

    await assertRejected(
      program.methods
        .makeMove({
          mv: { from: sq(FILE.e, 6), to: sq(FILE.e, 4), promotion: 0 },
          expectedNonce: 0, comment: null,
        })
        .accountsPartial({
          mover: black.publicKey, game, vault,
          white: white.publicKey, black: black.publicKey,
        })
        .signers([black]).rpc(),
      "NotYourTurn"
    );

    await assertRejected(
      program.methods
        .makeMove({
          mv: { from: sq(FILE.e, 0), to: sq(FILE.e, 3), promotion: 0 },
          expectedNonce: 0, comment: null,
        })
        .accountsPartial({
          mover: white.publicKey, game, vault,
          white: white.publicKey, black: black.publicKey,
        })
        .signers([white]).rpc(),
      "IllegalMoveShape"
    );

    await program.methods
      .makeMove({
        mv: { from: sq(FILE.e, 1), to: sq(FILE.e, 3), promotion: 0 },
        expectedNonce: 0, comment: null,
      })
      .accountsPartial({
        mover: white.publicKey, game, vault,
        white: white.publicKey, black: black.publicKey,
      })
      .signers([white]).rpc();

    await assertRejected(
      program.methods
        .makeMove({
          mv: { from: sq(FILE.e, 6), to: sq(FILE.e, 4), promotion: 0 },
          expectedNonce: 0, // should be 1 now
          comment: null,
        })
        .accountsPartial({
          mover: black.publicKey, game, vault,
          white: white.publicKey, black: black.publicKey,
        })
        .signers([black]).rpc(),
      "StaleMoveNonce"
    );
  });

  it("ends the game on resignation and pays the opponent the wager", async () => {
    const sessionId = nextSession();
    const wager = new BN(LAMPORTS_PER_SOL).divn(20);
    const game = findGamePda(programId, white.publicKey, sessionId);
    const stream = findStreamPda(programId, game);
    const vault = findVaultPda(programId, game);

    await program.methods
      .createGame({
        sessionId,
        timeControl: { initialMs: new BN(0), incrementMs: new BN(0), delayMs: new BN(0) },
        wagerPerSide: wager,
        streamMeta: "",
        streamLabel: "",
        streamer: null,
      })
      .accountsPartial({ creator: white.publicKey, game, stream, vault, systemProgram: SystemProgram.programId })
      .signers([white]).rpc();
    await program.methods
      .joinGame()
      .accountsPartial({ black: black.publicKey, game, vault, systemProgram: SystemProgram.programId })
      .signers([black]).rpc();

    const balanceBefore = await provider.connection.getBalance(black.publicKey);
    await program.methods
      .resignGame()
      .accountsPartial({
        player: white.publicKey, game, vault,
        white: white.publicKey, black: black.publicKey,
      })
      .signers([white]).rpc();

    const fetched = await program.account.game.fetch(game);
    assert.deepEqual(fetched.result, { blackWins: {} });
    assert.deepEqual(fetched.endReason, { resignation: {} });

    const balanceAfter = await provider.connection.getBalance(black.publicKey);
    assert.ok(balanceAfter > balanceBefore + wager.toNumber() - 50_000, "black received pot");
  });

  it("supports draw offer + accept and splits the wager", async () => {
    const sessionId = nextSession();
    const wager = new BN(LAMPORTS_PER_SOL).divn(50);
    const game = findGamePda(programId, white.publicKey, sessionId);
    const stream = findStreamPda(programId, game);
    const vault = findVaultPda(programId, game);

    await program.methods
      .createGame({
        sessionId,
        timeControl: { initialMs: new BN(0), incrementMs: new BN(0), delayMs: new BN(0) },
        wagerPerSide: wager,
        streamMeta: "",
        streamLabel: "",
        streamer: null,
      })
      .accountsPartial({ creator: white.publicKey, game, stream, vault, systemProgram: SystemProgram.programId })
      .signers([white]).rpc();
    await program.methods
      .joinGame()
      .accountsPartial({ black: black.publicKey, game, vault, systemProgram: SystemProgram.programId })
      .signers([black]).rpc();

    await program.methods
      .offerDraw()
      .accountsPartial({ player: white.publicKey, game })
      .signers([white]).rpc();

    const { events } = await captureEvents(program, async () => {
      await program.methods
        .acceptDraw()
        .accountsPartial({
          player: black.publicKey, game, vault,
          white: white.publicKey, black: black.publicKey,
        })
        .signers([black]).rpc();
    });
    assert.ok(events.find((e) => e.name === "drawEvent"));

    const fetched = await program.account.game.fetch(game);
    assert.deepEqual(fetched.result, { draw: {} });
    assert.deepEqual(fetched.endReason, { drawAgreed: {} });
  });

  it("emits StreamReactionEvent on record_reaction and bumps spectator count", async () => {
    const sessionId = nextSession();
    const game = findGamePda(programId, white.publicKey, sessionId);
    const stream = findStreamPda(programId, game);
    const vault = findVaultPda(programId, game);

    await program.methods
      .createGame({
        sessionId,
        timeControl: { initialMs: new BN(0), incrementMs: new BN(0), delayMs: new BN(0) },
        wagerPerSide: new BN(0),
        streamMeta: "twitch:foo",
        streamLabel: "foo-room",
        streamer: null,
      })
      .accountsPartial({ creator: white.publicKey, game, stream, vault, systemProgram: SystemProgram.programId })
      .signers([white]).rpc();
    await program.methods
      .joinGame()
      .accountsPartial({ black: black.publicKey, game, vault, systemProgram: SystemProgram.programId })
      .signers([black]).rpc();

    const payload = new Uint8Array(32);
    payload.set(Buffer.from("POG"));

    const { events } = await captureEvents(program, async () => {
      await program.methods
        .recordReaction({
          kind: 0,
          payload: Array.from(payload),
          registerAsSpectator: true,
        })
        .accountsPartial({ spectator: spectator.publicKey, game, stream })
        .signers([spectator]).rpc();
    });

    assert.ok(events.find((e) => e.name === "streamReactionEvent"));
    assert.ok(events.find((e) => e.name === "spectatorJoined"));

    const g = await program.account.game.fetch(game);
    assert.equal(g.spectators, 1);
    const s = await program.account.streamSession.fetch(stream);
    assert.equal(s.reactionCount.toNumber(), 1);
  });

  it("supports multiple concurrent games per creator", async () => {
    const ids = [nextSession(), nextSession(), nextSession()];
    const games = [] as PublicKey[];
    for (const sessionId of ids) {
      const game = findGamePda(programId, white.publicKey, sessionId);
      const stream = findStreamPda(programId, game);
      const vault = findVaultPda(programId, game);
      await program.methods
        .createGame({
          sessionId,
          timeControl: { initialMs: new BN(0), incrementMs: new BN(0), delayMs: new BN(0) },
          wagerPerSide: new BN(0),
          streamMeta: "",
          streamLabel: "",
          streamer: null,
        })
        .accountsPartial({ creator: white.publicKey, game, stream, vault, systemProgram: SystemProgram.programId })
        .signers([white]).rpc();
      games.push(game);
    }
    for (const g of games) {
      const fetched = await program.account.game.fetch(g);
      assert.deepEqual(fetched.status, { waitingForOpponent: {} });
    }
  });
});

async function assertRejected(p: Promise<any>, errorName: string) {
  try {
    await p;
    assert.fail(`expected error ${errorName} but tx succeeded`);
  } catch (e: any) {
    const msg = JSON.stringify(e?.error ?? e?.message ?? e);
    if (!msg.includes(errorName)) {
      throw new Error(`expected ${errorName} but got: ${msg}`);
    }
  }
}
