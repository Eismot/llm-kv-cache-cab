# Third-Party Licensing

## Project License

Cache Cab's original code and documentation use the [MIT License](LICENSE).
It permits personal and commercial use, modification, and redistribution, with
the license and copyright notice retained. Third-party components keep their own
licenses. MIT does not establish ownership of every model-generated output.

## Included Assets

The browser's five SVG icons come from [Lucide](https://lucide.dev/), under ISC.
The complete upstream notice, including the MIT notice for Feather-derived icons,
is preserved in [web/icons/LICENSE](web/icons/LICENSE). Do not remove it from
source distributions or icon-containing builds. Demo screenshots and GIFs also
display those icons; retain the attribution when redistributing those assets.

## Separately Obtained Tools And Weights

| Component | Reviewed terms | Repository boundary |
| --- | --- | --- |
| llama.cpp b11498 | [MIT / ggml authors](https://github.com/ggml-org/llama.cpp/blob/b11498/LICENSE) | Engine archive and executable are not bundled |
| Qwen2.5-Coder-1.5B-Instruct GGUF, pinned revision | [Apache-2.0 / Alibaba Cloud](https://huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF/blob/f86cb2c1fa58255f8052cc32aeede1b7482d4361/LICENSE) | Model weights are not bundled; README identifies the exact artifact |
| TinyStories probe model | Separately downloaded historical smoke-test artifact | Not bundled; this review does not certify its model/training-data provenance |

The application communicates with llama.cpp through a local HTTP API; its source
is not copied into this project. Qwen and engine names identify compatible tools,
not endorsement. Downloaded files stay in ignored .cache-cab storage.

If redistributing those tools or weights separately, obtain and retain their
licenses, copyright notices, and applicable NOTICE files. Modified Apache-licensed
files need modification notices. A root MIT file does not replace those duties.
Binary engine bundles may include additional libraries with separate terms.

## Rust And JavaScript Dependencies

Rust dependencies are pinned in Cargo.lock and obtained through Cargo rather than
vendored here. The reviewed locked metadata contains MIT, Apache-2.0, BSD-3-Clause,
Zlib, Unicode-3.0, and permissive alternative expressions including 0BSD, Unlicense,
BSL-1.0, and Apache-2.0 with LLVM exception. Alternative expressions allow a choice;
AND expressions require complying with each applicable part. The review found no
declared GPL/AGPL dependency in the resolved Rust metadata.

Optional browser tooling is pinned in package-lock.json. Playwright is developer
tooling; neither node_modules nor browser executables are included in this source
repository. Playwright uses Apache-2.0; its installed browser/tool distributions
carry their own third-party notices and are not relicensed by Cache Cab.

Dependencies remain under their upstream licenses even when linked into an MIT
application. Before publishing compiled binaries, collect the license texts and
required notices for the actual target's linked dependencies. Metadata expressions
alone are not a complete binary-distribution notice bundle.

## Review Scope

The source publication review checked the existing project license, retained icon
notices, pinned Qwen/llama.cpp license texts, Cargo license metadata, and Git ignore
boundaries. No incompatible license was identified for this source-only release.
No third-party notice was removed or replaced with MIT.

This is a practical licensing inventory, not legal clearance. It does not guarantee
model training-data provenance, originality of generated snippets, absence of patent
claims, or compliance for future artifacts. Review copied code, new dependencies,
model changes, and binary distributions on their own terms.
