---
schema: "tusker.domain/v7"
kind: "domain"
id: "eval"
project: "rag_pdf_extract"
title: "Eval"
status: "current"
summary: "Durable source of truth for Eval."
source_of_truth:
  - "knowledge/domains/eval/CANON.md"
canonical_files:
  - "INDEX.md"
  - "CANON.md"
created_at: "2026-05-29T17:58:14Z"
updated_at: "2026-05-29T17:58:14Z"
state_rev: "sha256:90437298ea91110cbd37b7f38603be246fc38e6d7289507c25044556ab999212"
---

# Eval

## Summary

Durable source of truth for Eval.

## Read This When

- You need current source-of-truth context for eval.
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
