# Supreme Court corpus benchmark — 2026-07-21

- Host: `Saravanans-MacBook-Pro.local` (arm64)
- CPU: `Apple M1 Pro`; logical cores: `8`
- Profile: `full-core`; scheduler: `normal`; Rayon workers: `8`
- Corpus: `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/benchmarks/corpus`; documents: `21`; max tokens: `350`
- Fetch/cache time: **40602.60 ms** (reported separately; excluded from every parse wall metric).

## Parse run 1

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113710 | 1533.00 | 23.58 | 0.26 | 74174.75 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111329 | 1317.22 | 19.09 | 5.55 | 84518.06 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40977 | 653.06 | 21.07 | 0.47 | 62746.14 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30190 | 546.21 | 26.01 | 0.50 | 55271.93 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102128 | 1150.60 | 16.92 | 0.67 | 88760.96 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89910 | 1123.58 | 43.21 | 0.09 | 80021.33 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41743 | 605.11 | 19.52 | 0.49 | 68983.85 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69533 | 885.40 | 44.27 | 0.09 | 78533.11 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258756 | 3052.61 | 37.69 | 0.11 | 84765.59 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81712 | 1071.06 | 18.79 | 0.21 | 76290.99 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116273 | 1389.62 | 34.74 | 0.11 | 83672.37 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85339 | 989.70 | 18.33 | 0.27 | 86227.30 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152464 | 1754.68 | 13.09 | 0.38 | 86890.11 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370454 | 4103.18 | 37.99 | 0.10 | 90284.67 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344502 | 4449.36 | 23.42 | 0.27 | 77427.24 | `9f72b79d2c6a2fa1` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19660 | 395.31 | 17.97 | 0.41 | 49733.30 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304719 | 3483.15 | 12.71 | 0.44 | 87483.67 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87252 | 1286.98 | 49.50 | 0.08 | 67796.14 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766148 | 9090.43 | 16.62 | 0.32 | 84280.69 | `f00e1f37cdf8fdb6` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21377 | 465.85 | 46.58 | 0.08 | 45888.32 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63268 | 908.64 | 17.82 | 0.43 | 69629.20 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3271444 chars, parse wall **40254.74 ms**, weighted **20.91 ms/page**, median **21.07 ms/page**, **0.45 MB/s**, **81268.55 chars/s**.

## Parse run 2

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113710 | 1359.57 | 20.92 | 0.30 | 83636.91 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111329 | 1300.32 | 18.85 | 5.62 | 85616.63 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40977 | 571.96 | 18.45 | 0.54 | 71642.83 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30190 | 528.95 | 25.19 | 0.52 | 57075.52 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102128 | 1152.36 | 16.95 | 0.67 | 88625.15 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89910 | 1101.79 | 42.38 | 0.10 | 81603.61 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41743 | 612.45 | 19.76 | 0.49 | 68157.39 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69533 | 886.25 | 44.31 | 0.09 | 78457.24 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258756 | 3050.12 | 37.66 | 0.11 | 84834.62 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81712 | 1028.97 | 18.05 | 0.22 | 79411.13 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116273 | 1375.48 | 34.39 | 0.11 | 84532.76 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85339 | 975.52 | 18.07 | 0.28 | 87480.47 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152464 | 1806.41 | 13.48 | 0.37 | 84401.50 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370454 | 4123.87 | 38.18 | 0.10 | 89831.64 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344502 | 4224.97 | 22.24 | 0.28 | 81539.43 | `9f72b79d2c6a2fa1` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19660 | 365.38 | 16.61 | 0.45 | 53806.84 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304719 | 3262.51 | 11.91 | 0.47 | 93400.08 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87252 | 1105.30 | 42.51 | 0.09 | 78939.32 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766148 | 9442.71 | 17.26 | 0.31 | 81136.50 | `f00e1f37cdf8fdb6` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21377 | 468.80 | 46.88 | 0.08 | 45599.22 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63268 | 882.08 | 17.30 | 0.44 | 71726.03 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3271444 chars, parse wall **39625.79 ms**, weighted **20.58 ms/page**, median **19.76 ms/page**, **0.45 MB/s**, **82558.46 chars/s**.

## Stability check

Two unchanged-tree parse runs; threshold is ±20.00% for both aggregate and every document. Output hashes must also match.

Result: **PASS**; aggregate delta **-1.56%**; maximum absolute document delta **14.12%**; normalized outputs: **identical**.

| filename | run 1 wall ms | run 2 wall ms | delta | output |
|---|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 1533.00 | 1359.57 | -11.31% | same |
| `1.pdf` | 1317.22 | 1300.32 | -1.28% | same |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 653.06 | 571.96 | -12.42% | same |
| `11. Roxann v Arun Sharma.pdf` | 546.21 | 528.95 | -3.16% | same |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 1150.60 | 1152.36 | 0.15% | same |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 1123.58 | 1101.79 | -1.94% | same |
| `14. Tomaso Bruno v State of UP.pdf` | 605.11 | 612.45 | 1.21% | same |
| `15. MC Mehta v UoI.pdf` | 885.40 | 886.25 | 0.10% | same |
| `16. BWSSB v A Rajappa.pdf` | 3052.61 | 3050.12 | -0.08% | same |
| `17. Sunil B Naik v Geowave Commander.pdf` | 1071.06 | 1028.97 | -3.93% | same |
| `18. Mardia Chemicals v UoI.pdf` | 1389.62 | 1375.48 | -1.02% | same |
| `19. CBSE v Aditya Bandopadyay.pdf` | 989.70 | 975.52 | -1.43% | same |
| `2. Devas v Antrix.pdf` | 1754.68 | 1806.41 | 2.95% | same |
| `20 RC Cooper v UoI.pdf` | 4103.18 | 4123.87 | 0.50% | same |
| `3. Gayatri Balasamy v ISG.pdf` | 4449.36 | 4224.97 | -5.04% | same |
| `4. Har Narain v Mam Chand.pdf` | 395.31 | 365.38 | -7.57% | same |
| `5. Vodafone v UoI.pdf` | 3483.15 | 3262.51 | -6.33% | same |
| `6. Gujarat Bottling v Coca Cola.pdf` | 1286.98 | 1105.30 | -14.12% | same |
| `7. Puttaswamy v UoI.pdf` | 9090.43 | 9442.71 | 3.88% | same |
| `8. Vishaka v State of Rajasthan.pdf` | 465.85 | 468.80 | 0.63% | same |
| `9. Anita Hada v Godfather Travels.pdf` | 908.64 | 882.08 | -2.92% | same |
