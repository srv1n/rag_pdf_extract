# Supreme Court corpus benchmark — 2026-09-09

- Host: `Saravanans-MacBook-Pro.local` (arm64)
- CPU: `Apple M1 Pro`; logical cores: `8`
- Profile: `pr3-structured-spans-off`; scheduler: `pr3-sequential`; Rayon workers: `1`
- Corpus: `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/benchmarks/corpus`; documents: `21`; max tokens: `512`
- Fetch/cache time: **0.00 ms** (reported separately; excluded from every parse wall metric).

## Parse run 1

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113736 | 719.67 | 11.07 | 0.56 | 158039.53 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111353 | 693.10 | 10.04 | 10.55 | 160659.94 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40986 | 282.22 | 9.10 | 1.09 | 145225.82 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30196 | 207.73 | 9.89 | 1.32 | 145360.89 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102152 | 680.95 | 10.01 | 1.13 | 150013.05 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89929 | 282.46 | 10.86 | 0.38 | 318380.88 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41753 | 269.35 | 8.69 | 1.10 | 155012.22 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69545 | 217.19 | 10.86 | 0.38 | 320203.32 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258817 | 891.93 | 11.01 | 0.37 | 290175.13 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81734 | 689.00 | 12.09 | 0.33 | 118627.38 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116297 | 373.96 | 9.35 | 0.40 | 310991.03 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85359 | 464.86 | 8.61 | 0.58 | 183622.98 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152508 | 1516.29 | 11.32 | 0.44 | 100579.73 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370554 | 1203.82 | 11.15 | 0.36 | 307816.12 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344681 | 2728.19 | 14.36 | 0.44 | 126340.49 | `f3cb9d02e514aa79` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19666 | 142.69 | 6.49 | 1.15 | 137818.91 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304804 | 2124.52 | 7.75 | 0.72 | 143469.49 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87273 | 283.73 | 10.91 | 0.37 | 307588.83 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766376 | 13615.57 | 24.89 | 0.22 | 56286.73 | `4c3d159d587655a3` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21382 | 75.78 | 7.58 | 0.49 | 282158.10 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63285 | 542.81 | 10.64 | 0.72 | 116587.81 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3272386 chars, parse wall **28005.83 ms**, weighted **14.55 ms/page**, median **10.64 ms/page**, **0.64 MB/s**, **116846.60 chars/s**.

