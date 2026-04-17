# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

InSyncBee is a cross-platform (Linux, Mac, Windows) Google Drive sync application, inspired by [Insync](https://www.insynchq.com/). It syncs files bidirectionally between local directories and Google Drive. Users can configure multiple independent sync pairs (local folder <-> Google Drive folder).

## Technical Stack

- **Backend:** Rust (Tokio async runtime, daemon architecture)
- **Frontend:** Tauri v2 + SolidJS + TypeScript
- **Database:** SQLite via rusqlite
- **Key crates:** notify (file watching), reqwest (HTTP), blake3 (hashing), fast_rsync (delta sync), fastcdc (chunking), oauth2 (Google auth)
- **Design goals:** Fast, lightweight, zero data loss, clear user feedback

## Project Status

Early stage — see `Vibecoding/Instructions.md` for the original requirements spec and `DESIGN.md` for the full design document including architecture, features, and development phases.
