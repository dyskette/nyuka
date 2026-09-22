# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Architecture decision records 0001–0018 in `docs/adr/`, with an index and a
  template.
- Cargo workspace scaffold: `domain`, `persistence`, `jobs`, `aidoku-runtime`,
  `packaging`, `api`.
- Frontend scaffold under `web/`: Vite 8, React 19, TanStack Router and Query,
  Biome, Lingui.
- `deny.toml` with a documented, time-boxed exception for RUSTSEC-2023-0071.
- Design tokens in `web/src/app/theme.css`, including four contrast
  corrections found by measuring the proposed palette against WCAG 2.2 AA.
