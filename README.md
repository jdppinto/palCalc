# PalCalc

**First ever fully hallucinated Palworld calculator**

This repo is a vibecoded, allucinatory implementation of Palworld breeding math. It is not a polished game companion — it is a mood-driven breed planner with serious vibes.

## Version
- current version: `0.1.1`
- release automation: push a tag like `v0.1.1` and GitHub creates a release automatically

## What’s new in this build
- Windows GitHub Actions pipeline builds the app with `npm run tauri build`
- release workflow creates a GitHub release on `v*` tags
- the README now clearly explains the build and release flow

## Features
- fully hallucinated Palworld breeding calculator
- uses Tauri + Svelte + Rust
- automatic Windows build via GitHub Actions
- automatic release creation via tag push

## Build locally

```bash
npm ci
npm run tauri build
```

## GitHub Actions

- `windows-tauri-build.yml` runs on pushes and PRs to `main`/`master`
- `windows-tauri-release.yml` runs on tag pushes like `v0.1.1`
- the release workflow creates a GitHub release and uploads `palcalc.exe`

## Notes

- this is a personal project, not a polished official guide
- if the app feels more vibe than math, that is intentional
- if you want a release, tag the branch and let GitHub Actions do the rest

Enjoy the hallucinated vibes.