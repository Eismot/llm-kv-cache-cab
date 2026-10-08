# Cache Cab

## Vision

Keep local coding sessions useful on everyday PCs without turning memory pressure
into repeated prompt processing or an unresponsive machine.

Cache Cab is a resource-aware session manager, not another coding agent and not
a promise that SSD storage performs like RAM. OpenCode or an editor remains the
client; an established inference engine remains responsible for model execution.

## First Hypothesis

A cache policy that considers recomputation cost can preserve an expensive coding
conversation while cheaper helper sessions come and go. Compare it with LRU under
the same budget and request trace. This heuristic is not globally optimal.

## MVP Contract

- Pure Rust scheduling and checkpoint storage, exercised through a local demo.
- The same scripted coding workload runs through LRU and cost-aware policies.
- RAM budgets and inference costs are explicitly modeled, not hardware measurements.
- Checkpoint file writes, integrity checks, and restoration are real.
- Checkpoints contain simulated session history, not LLM tensors or a KV cache.
- Cache misses restore or rebuild state; required computation is never skipped.
- No model downloads, cloud calls, shell execution, or edits to user repositories.

## Verified Engine Boundary

An opt-in live lab now saves and restores actual llama.cpp KV files through a
dedicated single-slot engine. The first tiny-model smoke test verifies binary
integrity, model/build/context identity, and exact token-prefix reuse. The app
does not download models; local test assets were fetched separately with approval.

The simulator's retention comparison remains modeled and is not connected to
real engine scheduling yet. The live panel reports one measured observation,
including checkpoint overhead. Fewer evaluated tokens are not automatically a
latency win, especially for the tiny smoke-test model.

## Evidence Before Claims

The demo explains the lifecycle; it does not establish an LLM speedup. A real
integration must compare equivalent model revisions, quantization, contexts,
sampling settings, and task quality. Measure time to first token, generation
latency, actual memory pressure, disk reads, and cold versus warm caches.

## Next Milestone

Connect the verified checkpoint adapter to session scheduling, extend runtime
configuration compatibility checks, and test one real coding workflow. The initial
adapter is for an unchanged dedicated engine configuration, not general reuse
across arbitrary KV formats, adapters, or runtime setting changes. Benchmark
restore versus rebuild before expanding policy or attempting expert scheduling.

## Open Decisions

- Is session switching measurably expensive in our real coding workload?
- When is restoring a checkpoint cheaper than rebuilding its prompt state?
- How should disk budgets and sensitive checkpoint data be managed?
- Should Cache Cab remain a companion service or become an upstream engine change?
