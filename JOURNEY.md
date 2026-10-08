# Cache Cab: From Idea to a Local Coding Demo

## The Starting Question

Could ordinary PCs use SSD storage and better caching to run useful local coding
models without needing a large GPU or lots of spare RAM?

We separated model capacity from interactive performance. Storage can make a model
fit, but compulsory weight reads can make generation slow. Existing engines already
offer quantization, memory mapping, placement, and caching; building another wrapper
does not establish an improvement.

## Choosing a Testable Project

OpenCode is primarily a coding agent. Ollama, LM Studio, and Lemonade manage and
serve local models. llama.cpp and mistral.rs implement inference. Cache Cab aims
to improve a specific session-management workload using an existing engine.

The first hypothesis was deliberately small: a policy considering recomputation
cost might preserve an expensive conversation when short helper requests arrive.
The project started as a separate local Git repository, with no publication.

## Milestone 1: Honest Simulation

We built a Rust cache core, atomic checksummed JSON checkpoints, localhost API,
and browser comparison. LRU and a cost-aware heuristic receive the same 12-request
route under the same logical budget.

At the default eight-unit budget, the main conversation is evicted by LRU and
retained by the cost-aware policy. Memory units and timings are modeled; JSON
writes and restoration are real. Those JSON files are not LLM KV state.

Desktop/mobile checks and browser screenshots produced a looping README GIF.
The simulation label stays visible. This demonstrates lifecycle and policy, not
a measured LLM performance gain.

## Milestone 2: Actual KV Checkpoints

With explicit download approval, we fetched a checksum-verified llama.cpp CPU build
and a tiny TinyStories 15M Q8 model. The dedicated engine uses one slot, bounded
context, and no automatic host prompt cache.

The Rust adapter performed actual model prefill, binary KV save, slot erase,
validated restore, and prefix reuse. Manifests bind state to a model hash, engine
build, effective slot context, and exact token prefix. Corrupt or incompatible
state is rejected before the restore request.

The verified tiny-model run saved and restored 284,244 bytes. Replaying the
41-token prompt evaluated one token and reused 40. A continuation then reused
41 prefix tokens while evaluating 11 new tokens.

We learned that llama.cpp's timing cache counter describes reused input; its
top-level cached-token count can describe post-request state instead. Measurements
must distinguish resident-model fresh KV tests from startup and OS-cold I/O.

Most importantly, fewer evaluated tokens did not automatically mean lower latency.
For the tiny model, checkpoint overhead could exceed fresh prompt processing.
The UI reports that result honestly rather than advertising an unsupported win.

## What We Had Not Built Yet

The tiny story model was a real local LLM, but not a coding model. The probe
processed prompts without generating a coding answer. Real checkpoint operations
were also separate from the modeled retention-policy comparison.

We had not demonstrated lower RAM use, faster coding, SSD-backed expert execution,
or production-grade agent behavior. We deliberately did not claim those results.

## Milestone 3: A Real Coding Conversation

This experiment introduced a small quantized Qwen2.5-Coder instruction model,
CPU-only, with bounded input and output. Larger weights require separate approval;
we do not close applications or change system settings to make them fit.

The implemented capture follows this sequence:

1. Show a small Rust function with an overflow problem.
2. Ask the local model for a refactoring and display its real output.
3. Validate only a tightly allowlisted, pure function in an isolated temporary folder.
4. Save actual conversation KV state and clear the dedicated engine slot.
5. Restore compatible state and ask a follow-up using its exact token prefix.
6. Display the actual answer, reuse counters, checkpoint bytes, and measured overhead.

Unexpected generated code is displayed, not executed. A failed answer is not replaced
with a scripted success. Sampling, prompts, model hashes, engine build, and output
limits belong in the report so the result can be investigated and reproduced.

With explicit approval, we downloaded the official Q4_K_M artifact at pinned
revision f86cb2c1fa58255f8052cc32aeede1b7482d4361: 1,117,320,768 bytes, SHA-256
cc324af070c2ecbfd324a30884d2f951a7ff756aba85cb811a6ec436933bb046.
The first transfer stopped partway; resuming it and checking the complete hash
prevented us from launching incomplete weights.

The model returned next_ticket(value: u32) -> Option<u32> using checked_add(1).
The full response was parsed with syn. Only the allowed pure AST was normalized
into an equivalent function and compiled in a temporary folder. Five fixed cases,
including u32::MAX, passed. This is not a sandbox for arbitrary model output.

We rendered the actual conversation with the engine's chat template, selected its
exact common token prefix with the follow-up, and re-prefilled that canonical
prefix before saving. The 3,414,596-byte checkpoint restored 119 state tokens.
The genuine follow-up reused those 119 prefix tokens and evaluated 39 new input
tokens, explaining that the overflow case returns None.

The recorded desktop response took about 2.13 seconds, save plus validation 156 ms,
restore 226 ms, and follow-up 2.34 seconds. These isolated observations are not
a speedup benchmark. Model hashing and canonical re-prefill add separate work.

The first live attempt exposed a stale pooled HTTP connection after large-model
hashing. Disabling idle connection reuse made the same test pass. Mobile checks
then caught wrapping-induced frame changes; stable status, validation, metric,
and footer dimensions fixed them without dropping the real output.

Desktop and mobile end-to-end tests now exercise the actual coding model, safe
sample validation, slot clearing, restoration, and mixed reused/new input counters.
The latest [coding GIF](assets/coding-demo.gif) uses four real UI states, five seconds
each, with processing waits omitted. The latest [measured report](assets/coding-report.json)
now records the expanded example below rather than the initial increment function.

## Milestone 4: More Complexity, Real Limitations

We kept the same running Qwen model and 2,048-token CPU slot, but replaced the
increment function with invoice_total: price times quantity, percentage discounts,
integer rounding, invalid-rate rejection, and multiple overflow boundaries.
The output cap increased from 192 to 256 tokens; no new model download was needed.

The AST boundary now accepts a small bounded pure subset: simple bindings,
comparisons, branches, checked arithmetic, and Option propagation. It still rejects
loops, macros, side-effecting calls, attributes, and extra items. Unlike the first
prototype, it compiles the actual accepted generated source, not a canonical
replacement. A deliberately wrong but safe function is accepted for compilation
and then fails the behavioral tests, proving safety is not confused with correctness.

The real model's refactor passed all 12 invoice cases in the temporary test project.
Its 7,459,604-byte checkpoint restored successfully, and the follow-up reused
260 prefix tokens while evaluating 78 new input tokens. Desktop and mobile checks
verified the answers, test report, exact-prefix restoration, and stable capture.

The follow-up exposed a real quality limitation. The model said (101, 3, 10)
returns 300, but the tested function returns Some(273). It also claimed the zero
discount case at u64::MAX overflows; it actually returns Some(u64::MAX).
Only the 50-percent case correctly returns None due to intermediate overflow.

We retained the model's original answer and displayed the compiled-test oracle
alongside it. No retries were used to curate a better answer. Passing generated-code
tests and restoring KV state do not establish that the model's explanations are
reliable. The current GIF and measured report preserve that distinction.

## Milestone 5: A Checkpointed Repository Feature

We added a small transparent harness rather than integrating a general coding
agent. It reads the actual coding_state handler from src/main.rs, records the
file hash, and asks the same local model to implement report-download status logic.
The model supplies the report_status function. Trusted harness code supplies the
HTTP attachment wiring, unique route insertion, and three fixed tests; these
roles are visible in the UI and patch plan.

The generated function must pass a strict AST boundary limited to boolean inputs,
branches, returns, and status literals. We compile its actual source in a disposable
repository copy, offline and with a bounded process timeout. Calls, loops, macros,
extra items, and arbitrary paths cannot enter that execution. The temporary copy
is not an OS sandbox, and this is deliberately not arbitrary agent-generated code.

The first proposal passed three API tests, including all eight status combinations.
We saved the canonical repository conversation, cleared the slot, generated an
unrelated HTTP helper answer in that same engine, and cleared it again. After
restoration, the model received the actual test/compiler feedback, retained its
correct policy, and passed the checks again. This demonstrated a context switch
and a feedback iteration, not a successful bug repair or an autonomous multi-file agent.

The recorded run saved/restored 10,615,284 bytes and reused 370 repository-prefix
tokens while processing 844 new feedback tokens. No generated edits were applied
to either working repository. Only the disposable copy received the proposed feature.

The first browser attempt exposed a startup race: Playwright's simulator fallback
occupied the port while the configured app was still building. Starting the compiled
configured app and verifying engine configuration before testing resolved it.
Desktop and mobile then passed the entire genuine model/checkpoint/test workflow.

The [repository GIF](assets/repository-demo.gif) contains five actual browser states,
five seconds each, with waits omitted. The [repository report](assets/repository-report.json)
preserves the source identity, model outputs, helper answer, patch plan, compiler
feedback, checkpoints, and measured counters. It is a reproducible narrow experiment,
not evidence of superior coding quality or lower end-to-end latency.

## Portfolio Rules

- Keep simulation and actual inference visibly separate.
- Capture the real UI and model output, not an invented typing animation.
- Prefer readable code, checkpoint transitions, and test evidence over decorative motion.
- Disclose shortened waits or changed playback timing; retain the full trace.
- Report restore overhead and unsuccessful validation as well as successful reuse.
- Link the GIF to source, run instructions, and the experiment's limitations.

## The Next Decision

After the coding demo works, collect a representative real coding-agent trace.
Decide whether checkpoint restoration is expensive enough to justify a better
retention policy. Benchmark equivalent models, quantization, contexts, and task
quality before connecting the simulated scheduler or claiming a resource win.
