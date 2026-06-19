---
schema: "tusker.domain/v7"
kind: "domain"
id: "docs"
project: "rag_pdf_extract"
title: "Docs"
status: "current"
summary: "Durable source of truth for Docs."
source_of_truth:
  - "knowledge/domains/docs/CANON.md"
canonical_files:
  - "INDEX.md"
  - "CANON.md"
created_at: "2026-05-29T17:58:14Z"
updated_at: "2026-05-29T17:58:14Z"
state_rev: "sha256:0a0653e98696e119178003646221156fa77ba608610c737440e3ca37f1b18833"
---

# Docs

## Summary

Durable source of truth for Docs.

## Read This When

- You need current source-of-truth context for docs.
- You are changing behavior owned by this domain.

## Canonical Files

- CANON.md - current durable truth.
- INDEX.md - domain map and routing hints.

## Runbooks

- _None yet._

## Interfaces

- _No stable interfaces declared yet._

## Invariants

- Keep durable truth in CANON.md.
- Put procedural guidance in runbooks/.

## Sources

- Raw external input belongs in sources/. Do not treat root docs/ or site output as canonical V7 knowledge.

## Glossary

- See glossary.md.

## Current Work

- _No current work linked._
