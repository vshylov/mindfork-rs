# Vendored syntax grammars

Sublime-syntax grammars for languages missing from syntect's bundled set (75
syntaxes, a snapshot of Sublime Text's defaults). They are compiled into the
binary's syntax dump by [`build.rs`](../build.rs); see
[docs/history/vendored-syntaxes.md](../docs/history/vendored-syntaxes.md) for why, and
[ADR 0003](../docs/decisions/0003-own-markdown-renderer.md) for the renderer
they serve.

**These files are vendored, not ours.** Each row below pins the upstream
repository, the exact commit the file was taken from, and the licence it is
redistributed under; the licence text itself is vendored next to the grammar in
`licenses/`. `python tools/fetch_syntaxes.py` re-fetches everything from these
pins (`--check` verifies the working copy against them without writing).

Two constraints decide what can appear here (both measured, see the plan §2.1):
syntect loads **only** `.sublime-syntax` — never `.tmLanguage` — and it does
**not** support `extends:`, so every grammar must be self-contained.

| Grammar | File | Repository | Path | Commit | Licence | Licence path |
|---|---|---|---|---|---|---|
| Zig | Zig.sublime-syntax | codeberg:ziglang/sublime-zig-language | Syntaxes/Zig.sublime-syntax | d3816b6d851f3c10ac74474e24b6414ce5b9ec0c | MIT | LICENSE |
| TypeScript | TypeScript.sublime-syntax | github:sharkdp/bat | assets/syntaxes/02_Extra/TypeScript.sublime-syntax | 73dc3258bec83bd7c66334f05b13c2d8221859f9 | Apache-2.0 | github:Microsoft/TypeScript-Sublime-Plugin@90441845a32ebcbd54632a39688ee24b2a2f8984:LICENSE |
| TOML | TOML.sublime-syntax | github:jasonwilliams/sublime_toml_highlighting | TOML.sublime-syntax | f3a8d6eb3c5fb9587ed992e384131b84c9619d6a | MIT | LICENSE |
| Dockerfile | Dockerfile.sublime-syntax | github:asbjornenge/Docker.tmbundle | Syntaxes/Dockerfile.sublime-syntax | c001fb280561d7c16f0f2837d76af493cf6c3bf8 | MIT | LICENSE |
| PowerShell | PowerShell.sublime-syntax | github:SublimeText/PowerShell | PowerShell.sublime-syntax | 2938700b586aeaa9c1d2dc0c04a1cc569ff3c24e | MIT | LICENSE.txt |
| Swift | Swift.sublime-syntax | github:colinta/decent-swift-syntax | Swift.sublime-syntax | 529205cb700bbaf65a624bc0616dc6713c868ec5 | MIT | LICENSE |
| Kotlin | Kotlin.sublime-syntax | github:guille/sublime-kotlin | Kotlin.sublime-syntax | 9b8b4a1f651ff651741fab031fbb6d6a4fce3ac3 | Unlicense | LICENSE.md |
| SCSS | SCSS.sublime-syntax | github:braver/SublimeSass | Syntaxes/SCSS.sublime-syntax | d3d94046409db6fbbc9d51dea52b589ecc9d3d48 | MIT | LICENSE |
| Sass | Sass.sublime-syntax | github:braver/SublimeSass | Syntaxes/Sass.sublime-syntax | d3d94046409db6fbbc9d51dea52b589ecc9d3d48 | MIT | LICENSE |
| GraphQL | GraphQL.sublime-syntax | github:dncrews/GraphQL-SublimeText3 | GraphQL.sublime-syntax | acc1dfe30dde5091069afd6070b79a68eb1c229a | WTFPL | LICENSE |
| Terraform | Terraform.sublime-syntax | github:alexlouden/Terraform.tmLanguage | Terraform.sublime-syntax | 6a31694f28ecca8a478fd9420a74955aa87e8c6c | MIT | LICENSE |
| Elixir | Elixir.sublime-syntax | github:princemaple/elixir-sublime-syntax | syntaxes/Elixir.sublime-syntax | b63f8f067d331c4415ff64998dca7ca535cbdb6d | MIT | LICENSE |
| Solidity | Solidity.sublime-syntax | github:davidhq/SublimeEthereum | Solidity.sublime-syntax | 60a7558416b19bd05870f49d5f9d6c033a52cf71 | MIT | LICENSE |
| Julia | Julia.sublime-syntax | github:JuliaEditorSupport/Julia-sublime | Julia.sublime-syntax | 3366b10be91aaab7a61ae0bc0a5af5cc375e58d1 | MIT | LICENSE |
| Nix | Nix.sublime-syntax | github:sharkdp/bat | assets/syntaxes/02_Extra/Nix.sublime-syntax | 73dc3258bec83bd7c66334f05b13c2d8221859f9 | MIT | github:wmertens/sublime-nix@48c497c709c66a2fb118c534a8de8e4e1c4c401d:LICENSE |
| Dart | Dart.sublime-syntax | github:sharkdp/bat | assets/syntaxes/02_Extra/Dart.sublime-syntax | 73dc3258bec83bd7c66334f05b13c2d8221859f9 | MIT | github:elMuso/Dartlight@2734901b014191f5a7f71c3f48678adf31239098:LICENSE |
| Protobuf | Protobuf.sublime-syntax | github:VcamX/protobuf-syntax-highlighting | Protobuf.sublime-syntax | 1365331580b0e4bb86f74d0c599dccc87e7bdacb | MIT | LICENSE |
| CMake | CMake.sublime-syntax | github:zyxar/Sublime-CMakeLists | CMake.sublime-syntax | 2560a080b3b7dcdb05e17ee85b2171776e84b8c1 | MIT | LICENSE |
| nginx | nginx.sublime-syntax | github:SublimeText/nginx | Syntaxes/nginx.sublime-syntax | f7a08a8c40caae3e423805dfe7b644d06c9ceff4 | MIT | LICENSE.md |
| Vue | Vue.sublime-syntax | github:vuejs/vue-syntax-highlight | Vue Component.sublime-syntax | 6eb71bc6bba5e6a284b6d1d3154484da6f366e21 | MIT | LICENSE |
| Svelte | Svelte.sublime-syntax | github:corneliusio/svelte-sublime | Svelte.sublime-syntax | c71f1290b061c79c027b5eb002ed06aa6d874ffe | MIT | LICENSE |
| Nim | Nim.sublime-syntax | github:nim-lang/NimLime | Syntaxes/Nim.sublime-syntax | 17b8287df06edaa47e8609403994a9224dc291c0 | MIT | LICENSE |

A licence path of the form `host:owner/repo@commit:path` points at a **different
repository** than the grammar — see below.

## Files taken from `sharkdp/bat`

TypeScript, Nix and Dart have no self-contained `.sublime-syntax` upstream: the
original packages ship `.tmLanguage`, which syntect cannot load.
[bat](https://github.com/sharkdp/bat) keeps converted `.sublime-syntax` copies,
and those are what we vendor. The conversion is bat's; the **grammar and its
licence remain the original author's**, so the licence column points at the
upstream package, not at bat:

- TypeScript — [Microsoft/TypeScript-Sublime-Plugin](https://github.com/Microsoft/TypeScript-Sublime-Plugin) (Apache-2.0)
- Nix — [wmertens/sublime-nix](https://github.com/wmertens/sublime-nix) (MIT)
- Dart — [elMuso/Dartlight](https://github.com/elMuso/Dartlight) (MIT)

## Notes on individual pins

- **SCSS** and **Svelte** are pinned to older commits (the ones bat uses).
  `braver/SublimeSass` and `corneliusio/svelte-sublime` at `master` fail to
  load — they have moved to sublime-syntax v2 features syntect does not
  implement.
- **Vue** comes from the `new` branch, where the grammar is called
  `Vue Component.sublime-syntax` (the space is why
  `tools/fetch_syntaxes.py` quotes the path). It declares itself as
  `name: Vue Component`, so a ` ```vue ` block reaches it via the `.vue` file
  extension rather than by name.
- **Zig** lives on Codeberg; the raw-file URL scheme differs from GitHub's and
  `tools/fetch_syntaxes.py` handles both.

## Languages deliberately not vendored

- **V (vlang)** — the only `.sublime-syntax` grammar in existence
  ([elliotchance/vlang-sublime](https://github.com/elliotchance/vlang-sublime))
  ships **no licence at all**: no `LICENSE` file, no statement in the README,
  which means all rights reserved. Redistributing it would be exactly the thing
  this manifest exists to prevent, so a ` ```v ` block falls back to the **Go**
  grammar instead (`canonical_lang`, chosen by measuring go/rust/c on real V
  code). If upstream ever adds a licence, vendoring it is one row here plus
  deleting that alias.
