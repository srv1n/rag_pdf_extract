# Supreme Court corpus benchmark — 2026-09-09

- Host: `Saravanans-MacBook-Pro.local` (arm64)
- CPU: `Apple M1 Pro`; logical cores: `8`
- Profile: `pr3-structured-spans-on`; scheduler: `pr3-sequential`; Rayon workers: `1`
- Corpus: `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/benchmarks/corpus`; documents: `21`; max tokens: `512`
- Fetch/cache time: **0.00 ms** (reported separately; excluded from every parse wall metric).

## Parse run 1

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113736 | 1746.49 | 26.87 | 0.23 | 65122.61 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111353 | 1757.07 | 25.46 | 4.16 | 63374.26 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40986 | 640.17 | 20.65 | 0.48 | 64023.44 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30196 | 437.34 | 20.83 | 0.63 | 69044.49 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102152 | 1628.07 | 23.94 | 0.47 | 62744.33 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89929 | 747.33 | 28.74 | 0.14 | 120334.41 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41753 | 584.49 | 18.85 | 0.51 | 71434.75 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69545 | 586.45 | 29.32 | 0.14 | 118586.98 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258817 | 2273.15 | 28.06 | 0.14 | 113858.38 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81734 | 1482.50 | 26.01 | 0.15 | 55132.59 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116297 | 1022.15 | 25.55 | 0.15 | 113777.32 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85359 | 1153.40 | 21.36 | 0.23 | 74006.65 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152508 | 3362.58 | 25.09 | 0.20 | 45354.45 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370554 | 3192.88 | 29.56 | 0.13 | 116056.25 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344681 | 4172.82 | 21.96 | 0.29 | 82601.53 | `f3cb9d02e514aa79` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19666 | 177.09 | 8.05 | 0.92 | 111048.24 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304804 | 2692.69 | 9.83 | 0.57 | 113196.77 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87273 | 432.65 | 16.64 | 0.24 | 201717.85 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766376 | 15089.69 | 27.59 | 0.19 | 50788.06 | `4c3d159d587655a3` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21382 | 111.27 | 11.13 | 0.33 | 192162.85 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63285 | 652.79 | 12.80 | 0.60 | 96945.81 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3272386 chars, parse wall **43943.05 ms**, weighted **22.83 ms/page**, median **23.94 ms/page**, **0.41 MB/s**, **74468.79 chars/s**.

