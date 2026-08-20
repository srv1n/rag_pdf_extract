# PDF Extract — narrative notes

Draft founder material for later curation in Brand HQ. These are story and motivation notes, not a fact sheet or marketing copy.

## The original itch

Sarav wanted to build a desktop application that could act as a context layer for LLMs. Developers can assemble PDF parsers and indexing pipelines from Python, Node, C++, and other ecosystems; a non-technical user, such as a 50-year-old lawyer, cannot reasonably be expected to do that setup. The desired experience was a desktop application that could ingest the user's documents and make them usable as LLM context.

The effort began while Sarav was building RZN Tauri. PDF Extract is the supporting library, not the desktop product itself.

## Why a fork became necessary

Recurring failures on legal PDFs forced the fork. The intended retrieval system combined BM25 with vector embeddings, which required documents to be divided into smaller contextual chunks. A fixed token- or character-count split can cut through a section arbitrarily and discard the document's implied organization.

PDF files encode visual placement rather than a dependable semantic hierarchy. They generally do not say “this row is a heading.” The fork therefore began inferring structure from signals such as row size, boldness, geometry, and surrounding layout, then using inferred section boundaries to choose better chunk breaks.

## Who it serves

The library was built for Sarav and is currently used only in his local RZN Backend and RZN Tauri products. Sarav is its only user. The eventual experience is aimed at non-technical people who need their documents available to an LLM without constructing an extraction and indexing stack themselves.

The public repository has never been promoted. Sarav reports that its single GitHub star is his own.

## What feels worth defending

Sarav is proud that the library can recover much of a document's apparent heading structure without sending every page through OCR. The point is practical rather than academic: geometric inference is intended to be cheap enough to run locally on a user's device while still producing chunks that respect more of the document's visible organization.

His current characterization is that the non-OCR path gets “90% of the way” for document understanding, is robust in release mode, is slower than PDFminer-style parsers, and is substantially faster and cheaper than OCR. These are founder judgments, not yet reproducible benchmark claims; retain the meaning, but do not publish the numbers or comparisons until a named corpus and commands support them.

## Honest limits

There is no external user story yet: Sarav alone uses the local applications. No single legal PDF or date was identified as the triggering incident; the fork came from repeated failures across legal documents. The “90%” assessment and comparisons with PDFminer and OCR remain experiential rather than measured. The repository's own evaluation history includes real failures and incomplete quality metrics, so the eventual story should emphasize the local-compute thesis and the extraction problems being attacked—not claim solved document understanding.
