# Fast PDF text extraction: knobs, override mode, and benchmark plan

This is a research/architecture question for the `rag_pdf_extract` Rust PDF extraction library. Inspect the supplied repository context and answer with evidence from the current code, not a speculative rewrite.

The requested outcome is a fast override mode for callers who want text quickly and do not need layout/font analysis. The desired behavior is: decode the PDF text layer, preserve reasonable stream/page order, normalize only what is necessary, split at sentence-ish boundaries (a naive splitter is acceptable), and return chunks/text with an explicit quality tradeoff. It should skip layout analysis and downstream layout-dependent work where safe. If an equivalent mode already exists, identify it precisely and explain what work it still performs.

Please answer:

1. Which current knobs and stages dominate latency (layout, `all_texts`, OCR, columns, tables, heading detection, tokenization, location/span emission, fallback double-pass, Rayon scheduling, etc.) and which knobs are safe high-payoff changes?
2. What is the smallest credible public API/configuration for a fast text-only override that preserves trust-boundary validation and error handling? Flag compatibility/location metadata consequences and any cases where the shortcut must refuse or fall back.
3. Give a benchmark design using this repository's existing corpus/examples. Include exact variants, warmup/repetition guidance, metrics, and acceptance gates. Do not invent measurements; state what must be measured.
4. Recommend a minimal implementation sequence, distinguishing verified facts from inference and unknowns. Avoid adding dependencies or a speculative rewrite.

Do not edit files. Cite repository paths and relevant functions. Keep the answer direct and evidence-led.
