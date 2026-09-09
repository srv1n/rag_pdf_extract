# Supreme Court corpus benchmark — 2026-09-09

- Host: `Saravanans-MacBook-Pro.local` (arm64)
- CPU: `Apple M1 Pro`; logical cores: `8`
- Profile: `pr3-structured-spans-on`; scheduler: `pr3-sequential`; Rayon workers: `1`
- Corpus: `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/benchmarks/corpus`; documents: `21`; max tokens: `512`
- Fetch/cache time: **0.00 ms** (reported separately; excluded from every parse wall metric).

## Parse run 1

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113736 | 888.07 | 13.66 | 0.46 | 128070.29 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111353 | 849.79 | 12.32 | 8.60 | 131036.41 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40986 | 347.60 | 11.21 | 0.88 | 117912.33 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30196 | 254.22 | 12.11 | 1.08 | 118781.19 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102152 | 840.13 | 12.35 | 0.92 | 121590.02 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89929 | 426.39 | 16.40 | 0.25 | 210907.65 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41753 | 333.73 | 10.77 | 0.89 | 125110.42 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69545 | 330.15 | 16.51 | 0.25 | 210645.77 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258817 | 1293.13 | 15.96 | 0.25 | 200147.44 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81734 | 774.77 | 13.59 | 0.29 | 105494.90 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116297 | 570.40 | 14.26 | 0.26 | 203885.11 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85359 | 599.02 | 11.09 | 0.45 | 142497.40 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152508 | 1756.72 | 13.11 | 0.38 | 86813.87 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370554 | 1824.21 | 16.89 | 0.24 | 203130.95 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344681 | 3654.34 | 19.23 | 0.33 | 94321.05 | `f3cb9d02e514aa79` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19666 | 174.51 | 7.93 | 0.94 | 112695.02 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304804 | 2601.09 | 9.49 | 0.59 | 117183.16 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87273 | 424.11 | 16.31 | 0.25 | 205779.61 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766376 | 14771.93 | 27.01 | 0.20 | 51880.56 | `4c3d159d587655a3` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21382 | 110.22 | 11.02 | 0.34 | 193997.06 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63285 | 640.96 | 12.57 | 0.61 | 98734.88 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3272386 chars, parse wall **33465.49 ms**, weighted **17.38 ms/page**, median **13.11 ms/page**, **0.54 MB/s**, **97783.89 chars/s**.

