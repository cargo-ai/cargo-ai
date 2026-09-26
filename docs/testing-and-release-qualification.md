# Testing and product qualification

[Documentation hub](./README.md) · [Cargo AI README](../README.md)

Development CI supplies automatic PR and `develop` feedback. Product Qualification is the on-demand umbrella for an exact candidate, with or without a release. Qualification never tags or publishes. Changes affecting runtime, installation, security or native behavior still need their selected deeper proof before integration.

## Entry points

| Workflow | Trigger and purpose |
|---|---|
| **Development CI** (`development-ci.yml`) | Automatic PR/`develop` and manual: Linux formatting, all-target compilation, binary units, interpreted providers, qualification-policy checks and security. PR checks use the prospective merge commit. This is not full native qualification. |
| **Core CI** (`multi-os-ci.yml`) | Manual/reusable: full credential-free tests, generated providers, build and local/packaged-source installation on Ubuntu, macOS and Windows. |
| **Package Qualification** (`package-qualification.yml`) | Manual/reusable: source-canary or official-package lifecycle on declared native platforms, with exact package selection and failure-only baseline diagnosis. |
| **Live Provider Conformance** (`live-provider-conformance.yml`) | Manually select one provider or all enrolled providers, using the same Rust probe and typed outcomes as the umbrella. Jev runs one bounded journey; the other providers retain bounded retries. Each job receives only its own protected key. |
| **Security Audit** (`security-audit.yml`) | Manual/reusable dependency audit, also called by Development CI and Product Qualification. |
| **Product Qualification** (`release-qualification.yml`) | Manual full umbrella: Core CI, packages, required/enrolled providers and security, followed by one fail-closed summary. No push, merge or weekly trigger. |
| **Registry Installation** (`registry-install.yml`) | Manual/reusable post-publication check: plain crates.io installation, source/version checks and smoke on all three OSes. No Core CI rerun or publication. |

The existing Windows rolling-preview artifact workflow is separate distribution automation; these changes do not alter it.

## Select a candidate

Select the workflow's branch/tag ref. Its triggering commit is captured for the run even if `develop` advances. For first-party qualification, `cargo_ai_sha` is an **expected SHA**: a different value fails before candidate code or provider execution. Select the matching branch/tag ref to test that revision. Protected providers retain their configured trusted-ref restrictions.

```bash
# Elective full qualification; replace the SHA placeholder.
gh workflow run release-qualification.yml --repo cargo-ai/cargo-ai \
  --ref develop -f cargo_ai_sha=<expected-develop-sha>

# Native family alone.
gh workflow run multi-os-ci.yml --repo cargo-ai/cargo-ai \
  --ref develop -f cargo_ai_sha=<expected-develop-sha>

# One provider with the same probe semantics as the umbrella.
gh workflow run live-provider-conformance.yml --repo cargo-ai/cargo-ai \
  --ref develop -f cargo_ai_sha=<expected-develop-sha> -f provider=openai

# Already-published source, independently of workflow implementation.
gh workflow run registry-install.yml --repo cargo-ai/cargo-ai \
  --ref develop -f cargo_ai_sha=<published-source-sha>
```

Package Qualification and Security Audit are selectable the same way. Package Qualification requires `cargo_ai_sha`; `official_package=true` selects the registered official package. A standalone pass certifies that family only. Full qualification launches each required family once; it needs no preceding standalone Core CI run.

Two provenance exceptions are intentional: external callers of the credential-free package workflow retain distinct workflow-implementation, Cargo AI and package commits; Registry Installation targets an explicitly selected published source commit, which can differ from its workflow revision. Interpret their results with those identities, not as a full release-tag check.

## How one product qualification run fits together

```text
exact Cargo AI candidate = first-party workflow-ref snapshot
  +-- Core CI: tests, generated parity, installation × Ubuntu, macOS, Windows
  +-- source-package lifecycle: declared native platforms
  +-- registered official-package lifecycle
  +-- live OpenAI and Anthropic + enrolled supplemental providers
  +-- security audit
  `-- product qualification summary: all required evidence must pass
```

Core CI runs three independent stages on each of Ubuntu, macOS and Windows: deterministic tests, generated-provider parity, and CLI/packaged-source installation. Each of the nine jobs has isolated state and a 60-minute limit; tests remain serial within each job. Every stage must pass. Core CI checks Cargo AI with loopback fixtures. Package families exercise independently maintained, allowlisted packages through build/package/install/inspect/run/hatch/uninstall. Their native OS coverage proves different behavior. Hosted-provider protocol checks run on Linux rather than multiplying paid calls across OSes. Existing optional-provider failure rules below remain unchanged.

The no-official-package baseline is sixteen required jobs: nine Core CI, three canary-package, two primary providers, security and summary. Each supplemental provider adds one job; the supported official package adds three. With all four supplemental providers and the official package, 23 jobs execute; redundant standalone family summaries are suppressed. Use the candidate catalog and actual run for applicability; missing required evidence is never a pass.

## Registry installation

After publication, run `registry-install.yml` against the published source SHA. It replaces Core CI's former `verify_registry_install` input. Plain `cargo install cargo-ai` runs in fresh isolated native environments with only registry, install-root, target and diagnostic options: no lock, version constraint or force flag. Verify the latest installed version and downloaded source commit, direct execution, Cargo dispatch and a credential-free local action. Installing a superseded version is not this check's contract.

Artifacts retain compiler-reported dependency identities/features, installation receipt, toolchain and crate/binary hashes. They describe the observed build, not an inferred graph or guarantee about future resolution. Workflow and published-source identities are recorded separately. Unpublished development qualification does not require registry installation.

## Release status

### 0.4.3 recorded results

Verified: 2026-09-22. [0.4.3](../releases/0.4.3.md) adds automatic local usage history and optional account backup/restore. The published, non-yanked crates.io archive embeds clean source `25bbe4705a771a612f0b3bd0ee2d019489bd5a20`, matching tag `v0.4.3`. Its SHA-256 is `c51b4a3f03ae82da92d7cbfb0e24204242064021dd40ec7162ce83d2b6d0ae54`, identical to the reviewed package. Later documentation commits do not change that released candidate.

[Product Qualification 35760660610](https://github.com/cargo-ai/cargo-ai/actions/runs/35760660610) passed on that exact source: nine parallel native stages, six source-canary/official-package cells, security and the aggregate summary. OpenAI, Anthropic, xAI and TypeSafe Jev live checks passed. Jev completed all eight interpreted/generated journey cases with eight requests and no retry; requested and returned model were `jev-1.13.0`. Gemini encountered two server errors then a rate limit; Mistral encountered three rate limits. Both remain supplemental **unverified**, not live-success passes. No all-provider or general model-judgment claim follows from aggregate success.

The earlier [qualification attempt](https://github.com/cargo-ai/cargo-ai/actions/runs/35742680595) timed out in the Windows installation stage after tests and generated parity passed. Its candidate remains unqualified. The corrected release separates native tests, generated parity and installation into independent jobs while retaining every check and the 60-minute limit.

Security audit passed with cargo-audit 0.22.1 and its freshly fetched advisory database. Separate `cargo deny --locked check` passed advisory, license, source and ban policy using RustSec revision `f7dc4b2860b29978f400fda0aab31cc4dbd21134`, retaining existing duplicate-version warnings. These are dated observations, not future security guarantees.

[Registry Installation 35786303701](https://github.com/cargo-ai/cargo-ai/actions/runs/35786303701) passed plain fresh registry installation on Linux, macOS and Windows, including exact archive/source/version, Cargo dispatch and a local action. A separate isolated macOS installation of the published binary confirmed schema-1 history capture for a successful local-only action; disabling tracking prevented new records while preserving existing facts. No provider or account request was made in that local check. The matching [GitHub release](https://github.com/cargo-ai/cargo-ai/releases/tag/v0.4.3) is published as Latest. Existing generated binaries require rebuilding/re-hatching to gain the new history behavior.

### 0.4.2 recorded results

Verified: 2026-09-21. [0.4.2](../releases/0.4.2.md) introduces TypeSafe Jev support. The published crates.io archive embeds clean source `3a71b3629405e042b704794944868ccf6e66122a`, matching tag `v0.4.2`. Its SHA-256 is `1de60a0c251fc17aac802d8af08b8a2867d51fdaeb1ca9cc10d9bb5e8a82ef39`, identical to the qualified package; the registry version is non-yanked.

[Product Qualification 35545290245](https://github.com/cargo-ai/cargo-ai/actions/runs/35545290245) passed on that exact source, including Core/native checks on Linux, macOS and Windows, source-canary and official-package qualification on all three platforms, security audit and the combined summary. The separate `cargo deny --locked check` passed dependency license, source, ban and advisory policy on the same source, retaining existing duplicate-version warnings. Security results describe the advisory data fetched for these checks, not future advisory status.

OpenAI (`gpt-5.6-luna`), Anthropic (`claude-sonnet-5`), Gemini (`gemini-3.7-flash`) and xAI (`grok-4.6`) passed live conformance. Mistral (`mistral-small-2603`) was rate-limited on all three bounded attempts and remains **unverified** under the supplemental-provider availability-warning policy; a successful aggregate is not a Mistral live pass.

A separate local macOS journey passed all eight bounded requests using `jev-1.13.0`: interpreted and hatched Choice/Score cases, fetched URL text and a live parent with a mocked child. These fixed fixtures establish the selected runtime journeys, not general model judgment or performance. That historical release did not include Jev in the hosted-provider GitHub aggregate. Current source supports explicit Jev enrollment as described below.

[Registry Installation 35552785215](https://github.com/cargo-ai/cargo-ai/actions/runs/35552785215) passed on Linux, macOS and Windows against published source `3a71b3629405e042b704794944868ccf6e66122a`; its workflow revision is separately recorded as the same commit. Each clean environment installed normally from crates.io and verified version, embedded source, direct/Cargo CLI dispatch and a local action. The matching [GitHub release](https://github.com/cargo-ai/cargo-ai/releases/tag/v0.4.2) was then published as Latest. Later documentation updates do not change this released source or claim qualification of another commit.

### 0.4.1 recorded results

README release badges and versioned links identify the released version; current `develop` CI is labeled separately. Verified: 2026-09-18. For `v0.4.1`, released commit `a81fda6d2cbbf3b7769f5f4abe38e139fc3eb521` passed [Core CI](https://github.com/cargo-ai/cargo-ai/actions/runs/35258805842) on Linux, macOS and Windows, including installed CLI smoke and freshly resolved packaged-source installation. The Core CI and package-installation badges represent those checks in the same run, not two separately executed suites or the catalog-based Package Qualification family.

The release's [Security Audit](https://github.com/cargo-ai/cargo-ai/actions/runs/35271269371) passed on integrated commit `fbe7d4067ea15fd0bc0bc7d93b25e6f50c6cb50a`. Its Git source tree is identical to the released commit (`git diff a81fda6d2cbbf3b7769f5f4abe38e139fc3eb521 fbe7d4067ea15fd0bc0bc7d93b25e6f50c6cb50a` is empty). The security badge is pinned to that immutable check revision and retains its original evidence identity; it is not a claim that a security check is attached to the release tag or that today's advisory database was tested. Full Product Qualification was not run for v0.4.1 and is not implied by these passing checks.

Verified: 2026-09-19. [Registry Installation 35443223970](https://github.com/cargo-ai/cargo-ai/actions/runs/35443223970) passed on Linux, macOS and Windows, including the combined summary. It installed the published v0.4.1 crate normally from crates.io and verified its version, embedded source `a81fda6d2cbbf3b7769f5f4abe38e139fc3eb521`, direct/Cargo CLI dispatch and local smoke action. Workflow implementation `8d8fe03952ba164300bb76323db5d1d62879b23d` is a separate identity from the published product. The link is specific to this completed run; no publication or full Product Qualification is implied.

After a qualified, published and verified release, update all version/tag/evidence references together. Filter Shields tag-check badges to actual named checks, such as `Product qualification summary`, and confirm the check commit equals the qualified release commit. Missing, pending, failed or unavailable results stay visible; never substitute unconditional green badges. Keep durable evidence/run links alongside badges. Non-release qualification does not advance release references.

## Local credential-free checks

Run these commands from the root of a public Cargo AI checkout. Keep local state away from your normal Cargo AI Home, disable keychain access, and serialize environment-mutating tests. On macOS or Linux, establish a disposable test environment first; use equivalent temporary environment settings on Windows:

```bash
export CARGO_AI_HOME="$(mktemp -d)/cargo-ai-home"
export CARGO_AI_DISABLE_KEYCHAIN=1
export RUST_TEST_THREADS=1
```

```bash
cargo test --locked --test qualification_policy
cargo test --locked --test product_conformance
cargo test --locked --test provider_smoke
cargo test --locked --test content_package_qualification
```

No command above needs a provider key or Cargo AI account credential, and each process test uses a temporary `CARGO_AI_HOME`. Qualification rules, report validation, summary rendering, package-catalog resolution and cache hashing run in Rust; this path requires no Python interpreter. The `qualification_policy` target builds and exercises the real maintainer-only `qualification-gate` Cargo example with synthetic inputs. The example is not an installed product command. CI retains YAML and bounded shell for checkout, job retrieval and routing.

The example routes `probe <provider>`, `aggregate`, `catalog` and `package-root` using the workflow’s explicit inputs. Only protected live jobs invoke probe mode. Aggregate mode requires candidate/run/job identity, matching dependency outputs and the candidate catalog before writing a passing summary. Catalog mode validates an allowlisted exact revision and a portable package-relative declaration before package checkout. Package-root mode validates the declared file beneath the anonymous checkout, rejects links/traversal and supplies its parent to both the candidate and diagnostic baseline. Helper/build/output failures remain failures.

## Maintainer provider testing

This section is for Cargo AI maintainers validating provider adapters. It is not part of end-user provider setup. Keep deterministic, generated, and live testing separate so a credential-free contract result is never confused with paid integration evidence.

### Deterministic interpreted adapters

The ordinary provider suite uses loopback fixtures, fake credentials, and a temporary Cargo AI Home. It covers interpreted success for OpenAI, Anthropic, Gemini, xAI, Mistral, TypeSafe Jev, and the Ollama-compatible transport, plus focused failure and capability boundaries where applicable:

```bash
cargo test --locked --test provider_smoke
```

To isolate one interpreted adapter, use its exact test name:

```bash
cargo test --locked --test provider_smoke interpreted_openai_smoke_isolated_and_deterministic -- --exact
cargo test --locked --test provider_smoke interpreted_anthropic_smoke_isolated_and_deterministic -- --exact
cargo test --locked --test provider_smoke interpreted_gemini_smoke_isolated_and_deterministic -- --exact
cargo test --locked --test provider_smoke interpreted_xai_smoke_isolated_and_deterministic -- --exact
cargo test --locked --test provider_smoke interpreted_mistral_smoke_isolated_and_deterministic -- --exact
cargo test --locked --test provider_smoke interpreted_ollama_smoke_isolated_and_deterministic -- --exact
cargo test --locked --test provider_smoke typesafe_smoke::interpreted_typesafe_public_example_maps_scores_and_drives_condition -- --exact
```

### Deterministic generated adapters

Generated-provider parity cases hatch and run complete standalone executables against the same loopback assertions. They are ignored by the ordinary Rust invocation because they compile full binaries. **Core CI** runs one sequential batch containing all seven provider cases plus the speech-generation/transcription-to-child chain on Ubuntu, macOS, and Windows:

```bash
cargo test --locked --test provider_smoke generated_provider_batch_isolated_and_deterministic -- --ignored --exact --nocapture
```

The batch prepares a neutral release seed, then copies its compatible template/compiler cache into each fresh case home. It preserves timestamps and executable permissions, verifies immutable seed SHA-256 digests in Rust, and transfers no credentials or provider-case state. Every generated application is still assembled and executed. The seven individually selectable cases remain available for diagnosis:

```bash
cargo test --locked --test provider_smoke generated_openai_smoke_isolated_and_deterministic -- --ignored --exact
cargo test --locked --test provider_smoke generated_anthropic_smoke_isolated_and_deterministic -- --ignored --exact
cargo test --locked --test provider_smoke generated_gemini_smoke_isolated_and_deterministic -- --ignored --exact
cargo test --locked --test provider_smoke generated_xai_smoke_isolated_and_deterministic -- --ignored --exact
cargo test --locked --test provider_smoke generated_mistral_smoke_isolated_and_deterministic -- --ignored --exact
cargo test --locked --test provider_smoke generated_ollama_smoke_isolated_and_deterministic -- --ignored --exact
cargo test --locked --test provider_smoke typesafe_smoke::generated_typesafe_smoke_isolated_and_deterministic -- --ignored --exact
```

These checks prove interpreted/generated parity for representative fixtures. They do not contact a provider, validate an account, or certify every model.

### Explicit live provider checkpoints

Live cases are separately ignored and must be intentional. Load the matching key and model into the shell environment through an approved secret mechanism, then run only the selected test:

| Provider | Required environment | Exact test |
| --- | --- | --- |
| OpenAI | `OPENAI_API_KEY`, `OPENAI_MODEL` | `live_openai_smoke_uses_isolated_stdin_credentials` |
| Anthropic | `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL` | `live_anthropic_smoke_uses_isolated_stdin_credentials` |
| Gemini | `GEMINI_API_KEY`, `GEMINI_MODEL` | `live_gemini_smoke_uses_isolated_stdin_credentials` |
| xAI | `XAI_API_KEY`, `XAI_MODEL` | `live_xai_smoke_uses_isolated_stdin_credentials` |
| Mistral | `MISTRAL_API_KEY`, `MISTRAL_MODEL` | `live_mistral_smoke_uses_isolated_stdin_credentials` |
| TypeSafe Jev | `TYPESAFE_API_KEY`, `TYPESAFE_MODEL` | `typesafe_smoke::live_typesafe_journey_uses_isolated_stdin_credentials` |

The TypeSafe journey makes at most eight inference requests with no automatic retries: three labeled Choice/Score messages interpreted and standalone, one fetched URL-text case and one live parent with a mocked child. It uses the existing isolated stdin credential store, omits unsupported generation settings and checks local marker behavior. It also asserts automatic history through `usage runs --json`, `usage show <run-id> --json` and `usage summary --json`, without `--usage-log` or `CARGO_AI_USAGE_LOG`. The standalone and Product Qualification workflows use this same journey when Jev is explicitly enrolled.

The separately ignored `live_media_bundle_uses_isolated_stdin_credentials` checkpoint exercises speech, transcription-to-child handoff and applicable image routes. Select `CARGO_AI_MEDIA_PROVIDER` (`openai`, `gemini`, `mistral` or `xai`), the corresponding API key, `OPENAI_API_KEY` and `OPENAI_MODEL` for the text child, and an existing private `CARGO_AI_MEDIA_PROOF_DIR` outside repositories. It requires `ffmpeg` and `ffprobe`; Mistral speech additionally requires an existing permitted `CARGO_AI_MISTRAL_VOICE_ID`. The directory retains attempt counts, sanitized diagnostics and media for content review. Preserve its ledger across runs, including earlier failed requests.

Optional `CARGO_AI_MEDIA_RUNTIME=current|hatched` and `CARGO_AI_MEDIA_CASE` select a narrower checkpoint. Cases are `speech-wav`, `chain`, `speech-mp3`, `image` and `image-reference`, subject to the provider's format/reference support. A selected `chain` reuses that runtime's prior WAV artifact. After diagnosing and correcting a recorded nonzero process result, `CARGO_AI_MEDIA_RETRY_FAILED=1` permits one retry of that failed case; successful or uncertain cases cannot be replayed. Retry evidence uses separate filenames and consumes the same finite request budget. These controls do not authorize provider calls or establish support without successful applicable evidence.

For Mistral transcription and image proof when no permitted saved voice exists, set `CARGO_AI_MEDIA_SKIP_SPEECH=1`. This runs only `chain` and `image` in the selected runtime, without requiring `CARGO_AI_MISTRAL_VOICE_ID`; a selected speech case is rejected. Set `CARGO_AI_MEDIA_INPUT_WAV` to an absolute path to an existing WAV inside `CARGO_AI_MEDIA_PROOF_DIR` for the `chain` case (for example, a retained synthetic speech artifact from another provider). The fixture copies that file into its isolated run directory and verifies a PCM codec, non-silent decoding, duration at most 20 seconds and size at most 10 MiB before reserving or sending the transcription request. The recorded chain artifact hash identifies the submitted WAV. The spoken content must contain the fixture's “blue lantern” and “silver compass” phrases for transcript and child checks to pass. An image-only selection needs no input WAV. The default Mistral bundle still requires a permitted voice and runs its speech cases.

After the selected provider's variables are already present, run its exact checkpoint:

```bash
cargo test --locked --test provider_smoke live_openai_smoke_uses_isolated_stdin_credentials -- --ignored --exact
cargo test --locked --test provider_smoke live_anthropic_smoke_uses_isolated_stdin_credentials -- --ignored --exact
cargo test --locked --test provider_smoke live_gemini_smoke_uses_isolated_stdin_credentials -- --ignored --exact
cargo test --locked --test provider_smoke live_xai_smoke_uses_isolated_stdin_credentials -- --ignored --exact
cargo test --locked --test provider_smoke live_mistral_smoke_uses_isolated_stdin_credentials -- --ignored --exact
```

Each live test writes its key into a temporary isolated profile through stdin. The key is not a process argument. Never place real credentials in source, agent JSON, command arguments, logs, artifacts, or a normal Cargo AI Home. Use an operator-selected model; a live pass is representative provider-wiring evidence, not a provider model catalog or per-model certification.

Ollama has no live-provider qualification job. The credential-free adapter checks its compatible transport; provisioning and managing a real local model server is outside the current qualification workflow.

## Public source-package qualification

Package repositories own their full unit, integration, and domain-specific suites. Cargo AI does not copy those tests into this repository. Instead, an enrolled repository exposes a small `cargo-ai-qualification.toml` data declaration with:

- one stable package id and build profile;
- one to three runner platforms;
- at most two representative run/hatch entrypoints;
- at most five structured checks with a program, argument array, relative working directory, and timeout.

Shell command strings, path traversal, embedded credentials, and secret requests are rejected. The central catalog distinguishes qualification canaries from official packages. A passing canary proves the harness; it never counts as an official package.

A declaration can live in a bounded subdirectory of the enrolled repository. The optional `image_findings_csv` entrypoint fixture names relative JSON response, image, expected CSV and SPDX inventory files. It tests that the actual image reaches the loopback provider and the installed tool writes the expected CSV; it never substitutes for live perception or protected provider proof. Mandatory lifecycle stages, platform coverage and time limits still apply. The standalone Package Qualification summary requires every declared platform to succeed; reusable callers retain `emit_summary` control and failures remain visible.


The public `cargo-ai/cargo-ai-qualification-canary` is the minimal real cross-repository fixture. The reusable workflow checks out both Cargo AI and package revisions by immutable commit, removes checkout credentials before package-controlled code runs, and emits JUnit plus sanitized provenance. Package build scripts and procedural macros still execute as code on a disposable runner, so only reviewed catalog entries are eligible.

The installed interpreted run proves the canary's structured Rust tool through the version-bound package runtime. The installed hatch compile check uses `--ignore-tools`: current hatch auditing resolves project source tools, while installed runtime tools are checked and executed by the interpreted package path. This is a stated coverage boundary, not a claim that hatch independently re-audits the installed runtime tool.

To run the external canary lifecycle locally after checking out both repositories:

```bash
CARGO_AI_QUALIFICATION_PACKAGE_ROOT=../cargo-ai-qualification-canary \
  cargo test --locked --test content_package_qualification \
  external_package_qualification_runs_mandatory_lifecycle -- --ignored --exact
```

## Exact revisions and failure attribution

The reusable workflow implementation commit, tested Cargo AI candidate commit, and package commit are separate provenance values. Callers pin the reusable workflow to a reviewed full commit and pass exact lowercase 40-character candidate/package commits.

Candidate failure always blocks. When an exact last product-qualified Cargo AI commit is supplied, only a failing package/OS cell runs one fresh inline baseline attempt. Candidate failure plus baseline success indicates a probable Cargo AI regression; both failing indicates a package or infrastructure suspect; no usable baseline remains unclassified. The diagnostic result never converts candidate failure into success.

## Hosted provider configuration

Use standalone families when commissioning access or diagnosing failures. Ordinary full qualification needs only Product Qualification; do not first rerun every family independently. OpenAI and Anthropic are required; enroll supplemental Gemini, xAI, Mistral or TypeSafe Jev explicitly. A single-provider run starts only the selected provider job and is integration evidence, not a Product Qualification decision. **Product Qualification** starts new direct jobs for required OpenAI and Anthropic plus every explicitly enrolled supplemental provider and remains the only complete aggregate gate.

The live workflow has no semantic dependency between providers:

```text
workflow dispatch
  +-- OpenAI (required)
  +-- Anthropic (selected directly or by all)
  +-- Gemini (when enrolled)
  +-- xAI (when enrolled)
  +-- Mistral (when enrolled)
  +-- TypeSafe Jev (when enrolled)
  `-- complete after every selected job finishes
```

Store these non-secret repository variables under **Settings → Secrets and variables → Actions → Variables**. Set a value to the exact lowercase string `true` to enroll that supplemental provider; leave it unset or set it to `false` to leave it unconfigured:

- `LIVE_GEMINI_ENABLED`
- `LIVE_XAI_ENABLED`
- `LIVE_MISTRAL_ENABLED`
- `LIVE_TYPESAFE_ENABLED`

The workflow never probes secret presence to infer enrollment. An invalid enrollment value fails visibly. Explicitly dispatching an optional provider also requires its enrollment variable to equal `true`.

Create a GitHub Environment named `live-provider-ci`. Store the required primary configuration there:

- `OPENAI_API_KEY`
- `OPENAI_MODEL` as a non-secret Environment variable
- `ANTHROPIC_API_KEY`
- `ANTHROPIC_MODEL` as a non-secret Environment variable

For each supplemental provider being enrolled, add only its matching Environment secret and non-secret model variable: `GEMINI_API_KEY`/`GEMINI_MODEL`, `XAI_API_KEY`/`XAI_MODEL`, `MISTRAL_API_KEY`/`MISTRAL_MODEL`, or `TYPESAFE_API_KEY`/`TYPESAFE_MODEL`. Select one representative hosted model per enrolled provider. Do not add an Ollama secret or model variable; real local-server provisioning is outside this workflow.

Restrict `live-provider-ci` to the trusted default branch and approved release tags. Before any provider key is injected, each focused or aggregate live job directly targets that Environment, requires an exact lowercase Cargo AI commit, and verifies that it equals the trusted triggering commit. Select the matching trusted workflow ref; an arbitrary older checkout cannot borrow a newer commit’s check status. It is intended to run unattended, so human release approval belongs to a separate `release-qualification` Environment attached only to the final aggregate summary.

The live tests write each selected key to a temporary isolated profile through stdin. Keep every key only as a `live-provider-ci` Environment secret; do not duplicate it into `release-qualification`, repository/organization secrets, workflow YAML, or inherited secret sets. Keys are not command arguments, logs, artifacts, caches, deterministic jobs, source-package jobs, other provider jobs, or the final summary. Missing OpenAI or Anthropic configuration and missing configuration for an explicitly selected or enrolled provider fail rather than silently skipping. Unenrolled optional providers are intentionally reported as not configured and do not block qualification.

## Evidence and release interpretation

Integration policy should require Development checks and Development security / Security audit on ordinary pull requests, plus any stronger native evidence selected for the change. Release readiness requires the protected Product qualification summary. Changing repository rules requires separate authorization; workflow edits alone do not configure protections. Package evidence records Cargo AI commit, optional baseline commit, package repository/commit, logical OS, runner image/version, architecture, declaration digest, workflow ref, classification, and result. It must never contain prompts, model output, tokens, raw provider bodies, Cargo AI Home state, or package runtime data.

The protected aggregate job writes the canonical human-readable dashboard directly to the GitHub Actions run summary. Open the Cargo AI repository, select **Actions**, select **Product Qualification**, and open a run's **Summary** page. When required proof passes, the headline says **Product Qualification: Passed**. The front table shows required product, package and primary-provider evidence; supplemental providers appear inside **Supplemental provider details**. Rows link to their producing jobs.

OpenAI and Anthropic must pass. An enrolled Gemini, Mistral, xAI or TypeSafe Jev probe may instead report **not verified — rate limited** when its completed isolated usage record positively classifies HTTP 429 as `ratelimited`. That qualified warning leaves its job, aggregate and overall workflow successful when every other obligation passes; the unsuccessful service result remains in collapsed details. Auth/configuration errors, malformed responses, unknown errors, timeouts, arbitrary 5xx responses, harness failures and missing/stale/cancelled evidence still block. Unenrolled supplemental providers are `not configured`, never passed. The legacy `LIVE_ANTHROPIC_ENABLED` flag cannot disable required Anthropic proof.

Qualification uses an explicit report mode and sanitized candidate/provider/run/attempt/probe-bound records. Standalone Live Provider Conformance and the umbrella use the same probe helper and policy: required providers must pass; a positively classified supplemental rate limit is reported as not verified. Direct live test invocations remain strict. The policy does not substitute providers or turn unavailable evidence into a model pass; the bounded retry behavior below applies to both workflow entry points. Earlier failed workflow runs retain their original conclusions.

Other dashboard states remain `pass`, `fail`, `cancelled`, `skipped`, and `missing`. The official-package row is `skipped` only while the validated catalog count is zero; enrolling an official package without aggregate results changes that row to `missing` and blocks release. JUnit and provenance artifacts remain the durable evidence behind the summary.

When GitHub reruns only failed jobs, the dashboard safely selects the newest completed attempt for each expected job from the same workflow run and exact candidate commit. A duplicate or mismatched result is treated as missing rather than guessed.

If a developer rejects the protected summary Environment or cancels the workflow before that job starts, GitHub's run status remains authoritative because no summary job ran to publish a dashboard.

This view is run-scoped and entirely GitHub-hosted. It requires no Cargo-AI.org page, GitHub Pages deployment, application server, database, custom API, or cross-system synchronization. The repository maintains only the workflow/reporting logic and normal GitHub Environment configuration.

The package catalog starts fail-closed until the public canary caller and its immutable commit are reviewed and pinned. Official-package count begins at zero. Adding packages increases only package shards; it does not multiply providers, models, or entrypoints.

---

[Documentation hub](./README.md) · [Cargo AI README](../README.md)

The official Animal Patrol source is enrolled separately from the source canary at an exact reviewed revision in the package catalog. Its image-to-CSV fixture requires all three native platforms. Catalog enrollment identifies source for qualification; it does not certify a hosted publication or replace the fresh Product Qualification decision.


### Retrying a qualification provider

Use GitHub Actions **Re-run jobs** for an individual provider job, or **Re-run failed jobs** to repeat failures and their dependent summary. Successful dependencies retain their results within the same workflow run and candidate. The summary shows the original evidence attempt; it rejects future attempts, another candidate/run, unsuccessful jobs and missing evidence. A new workflow run still starts fresh checks.

For OpenAI, Anthropic, Gemini, xAI and Mistral, the qualification gate makes at most three probe attempts for rate limits, HTTP 5xx, connection errors or timeouts, with 2- and 4-second delays. It does not retry invalid credentials, models, requests, response schemas or harness failures. Retry-After headers are not available in the current usage evidence; delays are fixed and bounded. Normal CLI and generated-agent inference do not gain automatic retries.

Each attempt records only typed outcome and error category in the job log and step summary. Final evidence retains the attempt history, and the aggregate identifies a pass after retry. Exhausted supplemental rate limits remain **unverified**, never a model pass; required provider failures still block qualification. Request bodies, raw provider errors and credentials are not published.


### Jev journey and automatic history

Select `typesafe` in Live Provider Conformance. Set repository variable
`LIVE_TYPESAFE_ENABLED=true`, place `TYPESAFE_API_KEY` only in the
`live-provider-ci` Environment, and set its `TYPESAFE_MODEL` variable to a
representative supported model (the existing fixture uses `jev-1.13.0`). The
optional `typesafe_model` dispatch input overrides that model; record the selected
value with the candidate. Environment branch restrictions still apply. Enrollment
and a stored secret do not establish that a key or model works.

The gate selects
`typesafe_smoke::live_typesafe_journey_uses_isolated_stdin_credentials` exactly once.
It compiles the standalone fixture before reading the live key, reserves at most
eight Jev request attempts, and stops at the first failure. A timeout consumes an
attempt; neither the inference nor the entire journey is automatically retried.
Rerunning a workflow starts a new paid journey and needs an intentional budget
decision. Other providers' retry behavior is unchanged.

A pass requires all eight capability cases and automatic local-history assertions:
each interpreted or standalone request has distinct identity, provider/profile and
requested/returned model attribution, available input/output usage, a valid total,
timing and completed status. The mocked child is separately attributed and linked
to its parent; summary totals count its request once. Repeated CLI JSON queries
must preserve the same facts. This tests a representative model and journey,
not the entire model catalog, SQLite concurrency, cloud backup or model quality
beyond the frozen labeled cases.

Public evidence contains only candidate/workflow/probe identity, bounded sanitized
model identifiers, started-request and completed-case counts, and typed outcomes.
Raw history, credentials, prompts, outputs and tool payloads are not uploaded.
A positively classified HTTP 429 can produce the supplemental **not verified —
rate limited** warning; completed earlier cases remain partial evidence and the
journey is not a live pass. Missing, malformed or inconsistent evidence and failed
capability/history assertions block qualification. Direct test invocations remain
strict even when a workflow's supplemental rate-limit policy permits a warning.
