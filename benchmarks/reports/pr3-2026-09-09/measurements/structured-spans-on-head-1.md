# Supreme Court corpus benchmark — 2026-09-09

- Host: `Saravanans-MacBook-Pro.local` (arm64)
- CPU: `Apple M1 Pro`; logical cores: `8`
- Profile: `pr3-structured-spans-on`; scheduler: `pr3-sequential`; Rayon workers: `1`
- Corpus: `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/benchmarks/corpus`; documents: `21`; max tokens: `512`
- Fetch/cache time: **0.00 ms** (reported separately; excluded from every parse wall metric).

## Parse run 1

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113736 | 954.11 | 14.68 | 0.43 | 119206.57 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111353 | 878.44 | 12.73 | 8.32 | 126761.70 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40986 | 349.89 | 11.29 | 0.88 | 117138.43 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30196 | 252.88 | 12.04 | 1.08 | 119409.69 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102152 | 848.51 | 12.48 | 0.91 | 120389.77 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89929 | 426.83 | 16.42 | 0.25 | 210690.40 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41753 | 475.27 | 15.33 | 0.63 | 87850.82 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69545 | 424.36 | 21.22 | 0.20 | 163883.93 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258817 | 1308.50 | 16.15 | 0.25 | 197797.19 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81734 | 808.68 | 14.19 | 0.28 | 101071.12 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116297 | 665.26 | 16.63 | 0.23 | 174815.06 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85359 | 663.44 | 12.29 | 0.41 | 128660.82 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152508 | 1875.81 | 14.00 | 0.36 | 81302.45 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370554 | 1789.15 | 16.57 | 0.24 | 207112.11 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344681 | 3681.31 | 19.38 | 0.32 | 93629.89 | `f3cb9d02e514aa79` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19666 | 175.78 | 7.99 | 0.93 | 111878.59 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304804 | 2599.30 | 9.49 | 0.59 | 117263.87 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87273 | 439.69 | 16.91 | 0.24 | 198487.27 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766376 | 14801.51 | 27.06 | 0.20 | 51776.87 | `4c3d159d587655a3` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21382 | 107.25 | 10.73 | 0.35 | 199358.84 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63285 | 647.10 | 12.69 | 0.60 | 97797.63 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3272386 chars, parse wall **34173.08 ms**, weighted **17.75 ms/page**, median **14.19 ms/page**, **0.53 MB/s**, **95759.19 chars/s**.

