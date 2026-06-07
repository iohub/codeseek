# Contributing to CodeSeek

Thanks for taking the time to contribute!

## Before you start

- Search [existing issues](https://github.com/CodeBendKit/codeseek/issues) to avoid duplicates.
- For bugs, include **OS**, **codeseek version** (`codeseek -V`), and reproduction steps.
- For feature requests, explain the use case and why it matters.

## Issue templates

### Bug report

```
### Environment
- OS: [e.g. macOS 15, Ubuntu 24.04]
- codeseek version: [e.g. 0.1.17]
- Install method: [npm / brew / from source]

### Steps to reproduce
1.
2.
3.

### Expected behavior

### Actual behavior

### Logs / output
```

### Feature request

```
### Problem
What problem are you trying to solve?

### Proposed solution
What would you like codeseek to do?

### Alternatives considered
Have you tried any workarounds?

### Additional context
```

### Language / parser bug

```
### Language
[e.g. TypeScript, Go]

### Code snippet
```...```

### Expected parsed symbols

### Actual result
```

## Development setup

```bash
git clone https://github.com/CodeBendKit/codeseek.git
cd codeseek
./build.sh --release
```

- Rust source: `rust-core/`
- TypeScript wrapper: `src/`
- Tests: `cargo test` in `rust-core/`
- Build: `./build.sh --release`

## Pull request checklist

- [ ] Build passes: `cd rust-core && cargo build`
- [ ] Tests pass: `cd rust-core && cargo test`
- [ ] TypeScript compiles: `npm run build`
- [ ] No new warnings
- [ ] Commit message follows [conventional commits](https://www.conventionalcommits.org/)

## Commit conventions

We use conventional commits:

```
feat: add batch embedding for 20x faster indexing
fix: search returns empty when LanceDB unavailable  
docs: update README with search pipeline
perf: reduce embedding API calls with batching
refactor: centralize version in package.json
chore: bump version to 0.1.18
```

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
