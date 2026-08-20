# Supreme Court corpus benchmark — 2026-07-21

- Host: `Saravanans-MacBook-Pro.local` (arm64)
- CPU: `Apple M1 Pro`; logical cores: `8`
- Profile: `efficiency-cores`; scheduler: `taskpolicy -c background`; Rayon workers: `8`
- Corpus: `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/benchmarks/corpus`; documents: `21`; max tokens: `350`
- Fetch/cache time: **1768.30 ms** (reported separately; excluded from every parse wall metric).

## Parse run 1

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113710 | 13546.64 | 208.41 | 0.03 | 8393.96 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111329 | 11998.95 | 173.90 | 0.61 | 9278.23 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40977 | 7063.68 | 227.86 | 0.04 | 5801.09 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30190 | 5217.61 | 248.46 | 0.05 | 5786.17 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102128 | 9982.19 | 146.80 | 0.08 | 10231.03 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89910 | 9126.13 | 351.00 | 0.01 | 9851.93 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41743 | 4195.32 | 135.33 | 0.07 | 9949.90 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69533 | 6912.50 | 345.63 | 0.01 | 10059.02 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258756 | 24649.64 | 304.32 | 0.01 | 10497.35 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81712 | 9394.96 | 164.82 | 0.02 | 8697.43 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116273 | 12550.33 | 313.76 | 0.01 | 9264.53 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85339 | 13046.33 | 241.60 | 0.02 | 6541.23 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152464 | 19167.11 | 143.04 | 0.04 | 7954.46 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370454 | 43471.16 | 402.51 | 0.01 | 8521.83 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344502 | 62705.82 | 330.03 | 0.02 | 5493.94 | `9f72b79d2c6a2fa1` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19660 | 5617.21 | 255.33 | 0.03 | 3499.96 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304719 | 23885.16 | 87.17 | 0.06 | 12757.67 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87252 | 8128.27 | 312.63 | 0.01 | 10734.39 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766148 | 56702.03 | 103.66 | 0.05 | 13511.83 | `f00e1f37cdf8fdb6` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21377 | 2951.24 | 295.12 | 0.01 | 7243.39 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63268 | 6212.26 | 121.81 | 0.06 | 10184.38 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3271444 chars, parse wall **356524.55 ms**, weighted **185.21 ms/page**, median **241.60 ms/page**, **0.05 MB/s**, **9175.93 chars/s**.

