# LLM KV Cache Cab

[![CI](https://github.com/Eismot/llm-kv-cache-cab/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Eismot/llm-kv-cache-cab/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Rust 1.81+](https://img.shields.io/badge/Rust-1.81%2B-orange?logo=rust)](Cargo.toml)

A Rust experiment in saving and restoring local LLM conversation state with
llama.cpp. Save a task's KV state, use the same engine slot for another task,
then restore the first conversation and continue.

The demo UI and internal Rust crate retain the shorter **Cache Cab** name.

## What You Can Try

| Experiment | What you see | What's real |
| --- | --- | --- |
| Cache policy | The same request route under LRU and cost-aware retention | JSON storage; memory units and timings are simulated |
| KV checkpoint | Save, clear, validate, restore, and continue a model prompt | Binary llama.cpp KV state, integrity checks, and engine counters |
| Coding conversation | A Rust invoice refactor, edge-case tests, and a restored follow-up | Local Qwen output and tests on accepted generated source |
| Repository task | Read source, propose feature logic, test, switch tasks, restore, and revise | Actual handler context, a temporary project, API checks, and prefix reuse |

The repository demo includes source context, a model proposal, temporary-project
tests, a helper request, and a restored follow-up. The UI shows the model output,
checkpoint sizes, test results, and reused/new input tokens.

![Actual repository context, model feature policy, offline tests, checkpointed helper switch, and restored revision](assets/repository-demo.gif)

Five actual browser states; waits omitted, five seconds per state. This is not
real-time playback. See the [repository-task report](assets/repository-report.json).

## Run

Requires Rust 1.81+ and Cargo. The default simulator needs no model or Node.js.
The compiled browser asset is included in the repository.

```sh
cargo run --locked
```

Open `http://127.0.0.1:7331`. Run route plays the simulation; the arrow steps through
one request. Changing the budget resets both policies. Use `--port=7333` if needed.

## Local Model

The coding and repository views use llama.cpp b11498 with this pinned
[Qwen2.5-Coder-1.5B-Instruct Q4_K_M artifact](https://huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF/tree/f86cb2c1fa58255f8052cc32aeede1b7482d4361).
The file is 1,117,320,768 bytes. SHA-256:

```text
cc324af070c2ecbfd324a30884d2f951a7ff756aba85cb811a6ec436933bb046
```

Download the engine and model separately. Create a private slot-save directory,
then start a dedicated CPU server and Cache Cab:

```sh
llama-server --model /absolute/qwen2.5-coder-1.5b-instruct-q4_k_m.gguf --host 127.0.0.1 --port 7332 --parallel 1 --ctx-size 2048 --threads 4 --threads-batch 4 --device none --cache-ram 0 --slots --slot-save-path /absolute/coding/slots/ --no-context-shift
cargo run --locked -- --engine=http://127.0.0.1:7332 --slot-dir=/absolute/coding/slots
```

Use the same absolute slot directory for both processes, with native Windows paths
on Windows. Keep weights and checkpoints under ignored .cache-cab storage. Do not
connect an engine serving other clients: the demos clear its only slot.

Open `/?view=coding` for the invoice example or `/?view=repository` for the source
task. Reset the active experiment before switching views. The standalone probe is:

```sh
cargo run --locked --bin engine_probe -- --endpoint=http://127.0.0.1:7332 --checkpoint-dir=/absolute/coding/slots --output=report.json
```

## Recorded Results

| Run | KV bytes restored | Reused / new input tokens | Checks |
| --- | ---: | ---: | --- |
| [Invoice](assets/coding-report.json) | 7,459,604 | 260 / 78 | 12 arithmetic cases passed |
| [Repository](assets/repository-report.json) | 10,615,284 | 370 / 844 | 3 API tests passed before and after restoration |

The invoice code passed, but the model's explanation got two cases wrong. The UI
keeps its answer beside the test oracle. The repository policy passed on the first
attempt; the restored model retained it. There was no failing-test repair iteration.

These are individual observations, not benchmarks. Save/restore, model hashing,
and canonical transcript re-prefill add work. Fewer evaluated tokens do not by
themselves mean lower latency. No speedup or RAM-saving claim has been established.

## Boundaries

- The policy comparison uses modeled budgets and timings, not real engine scheduling.
- Repository context is limited to the `coding_state` handler in src/main.rs.
- The model writes a status-policy function; the harness supplies HTTP wiring and tests.
- Generated code runs only after a narrow AST allowlist accepts it. The temporary project is not an OS sandbox.
- Repository tests run offline, with a locked dependency set, two build jobs, and a 180-second timeout.
- No generated changes are applied to the working repository.
- Checkpoints bind the model hash, engine build, context budget, and exact token prefix. Checksums detect corruption, not malicious replacement of both state and manifest.
- Public compiler paths are redacted. Review new artifacts before sharing them.

See [JOURNEY.md](JOURNEY.md) for the experiments and [VISION.md](VISION.md) for the
next research questions.

## Verify

```sh
cargo test --locked --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

Tests cover cache budgets, checkpoint integrity, storage failures, engine protocol,
localhost API boundaries, code review, and GIF encoding.

For TypeScript development and browser checks, install Node.js 20+:

```sh
npm ci --ignore-scripts
npm run typecheck
npm run build:web
npx playwright install chromium
npm run test:browser
```

Edit web/app.ts and rebuild web/generated/app.js before compiling Rust. The generated
JavaScript is committed so users can run the app without Node.js. CI checks types,
browser behavior, and whether that asset matches its TypeScript source.

Normal browser tests use the simulator and recorded report fixtures, not inference.
Live checks require the configured dedicated engine and one of
`CACHE_CAB_LIVE_TEST=1`, `CACHE_CAB_CODING_TEST=1`, or `CACHE_CAB_REPOSITORY_TEST=1`.

## Capture The Demo

With the configured app already running:

```powershell
$env:CACHE_CAB_REPOSITORY_TEST = "1"
npm run test:browser -- --project=desktop --grep "opt-in repository feature"
cargo run --locked --bin capture_gif -- captures/repository assets/repository-demo.gif 500
```

The encoder's last argument is frame delay in centiseconds. The shown GIF uses
five seconds per state and omits processing waits. A first offline build can take
minutes. Frames and traces go under the ignored captures directory.

The [invoice capture](assets/coding-demo.gif) and [policy simulation](assets/demo.gif)
are also included. `npm run capture:frames` recreates the simulation frames.

## Attribution

Our code uses [MIT](LICENSE). [Lucide](https://lucide.dev/) notices are retained in
[web/icons/LICENSE](web/icons/LICENSE). Dependencies, engine binaries, and model
weights keep their own terms; see [THIRD_PARTY.md](THIRD_PARTY.md).
