# Supreme Court corpus benchmark — 2026-09-09

- Host: `Saravanans-MacBook-Pro.local` (arm64)
- CPU: `Apple M1 Pro`; logical cores: `8`
- Profile: `pr3-structured-spans-on`; scheduler: `pr3-sequential`; Rayon workers: `1`
- Corpus: `/Users/sarav/Downloads/side/rzn/rag_pdf_extract/benchmarks/corpus`; documents: `21`; max tokens: `512`
- Fetch/cache time: **0.00 ms** (reported separately; excluded from every parse wall metric).

## Parse run 1

| filename | bytes | pages | chars | wall ms | ms/page | MB/s | chars/s | normalized output |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `1. Energy Watchdog v CERC.pdf` | 405704 | 65 | 113736 | 892.75 | 13.73 | 0.45 | 127399.77 | `cabddc423dc1c381` |
| `1.pdf` | 7310114 | 69 | 111353 | 850.75 | 12.33 | 8.59 | 130888.31 | `a4f8fbcb10b79740` |
| `10. Parswanath Saha v Bandhana Modak.pdf` | 307312 | 31 | 40986 | 347.79 | 11.22 | 0.88 | 117846.26 | `c0c922d9ab8ac40b` |
| `11. Roxann v Arun Sharma.pdf` | 274193 | 21 | 30196 | 266.41 | 12.69 | 1.03 | 113343.65 | `c75945582509a170` |
| `12. CCI v Kerala Film Exhibitors Federation & Ors.pdf` | 772345 | 68 | 102152 | 853.40 | 12.55 | 0.91 | 119700.60 | `ad8b75be19dc92ee` |
| `13. Kumari Shrilekha Vidyarthi v State of UP.pdf` | 105975 | 26 | 89929 | 427.90 | 16.46 | 0.25 | 210165.12 | `29af07dcc5671c83` |
| `14. Tomaso Bruno v State of UP.pdf` | 297154 | 31 | 41753 | 335.28 | 10.82 | 0.89 | 124533.08 | `f451d6c3c370c130` |
| `15. MC Mehta v UoI.pdf` | 82952 | 20 | 69545 | 329.63 | 16.48 | 0.25 | 210976.26 | `2b9f9a5057632795` |
| `16. BWSSB v A Rajappa.pdf` | 329348 | 81 | 258817 | 1292.53 | 15.96 | 0.25 | 200241.05 | `71c9affd0b96009c` |
| `17. Sunil B Naik v Geowave Commander.pdf` | 225921 | 57 | 81734 | 778.39 | 13.66 | 0.29 | 105004.26 | `9b257411441d9098` |
| `18. Mardia Chemicals v UoI.pdf` | 150263 | 40 | 116297 | 570.25 | 14.26 | 0.26 | 203941.70 | `3802776a48dfdb13` |
| `19. CBSE v Aditya Bandopadyay.pdf` | 270770 | 54 | 85359 | 601.99 | 11.15 | 0.45 | 141795.06 | `82d71433e34b406d` |
| `2. Devas v Antrix.pdf` | 671248 | 134 | 152508 | 1773.01 | 13.23 | 0.38 | 86016.19 | `f6063a021e261d98` |
| `20 RC Cooper v UoI.pdf` | 430257 | 108 | 370554 | 1812.55 | 16.78 | 0.24 | 204437.60 | `900c0c7c5a9d4c79` |
| `3. Gayatri Balasamy v ISG.pdf` | 1191356 | 190 | 344681 | 3751.26 | 19.74 | 0.32 | 91884.08 | `f3cb9d02e514aa79` |
| `4. Har Narain v Mam Chand.pdf` | 163756 | 22 | 19666 | 175.10 | 7.96 | 0.94 | 112310.29 | `0bc5cb89ce461e22` |
| `5. Vodafone v UoI.pdf` | 1524729 | 274 | 304804 | 2592.32 | 9.46 | 0.59 | 117579.53 | `62ac048448856714` |
| `6. Gujarat Bottling v Coca Cola.pdf` | 104232 | 26 | 87273 | 421.38 | 16.21 | 0.25 | 207113.78 | `56decfcb4596cf52` |
| `7. Puttaswamy v UoI.pdf` | 2930333 | 547 | 766376 | 14686.86 | 26.85 | 0.20 | 52181.05 | `4c3d159d587655a3` |
| `8. Vishaka v State of Rajasthan.pdf` | 37252 | 10 | 21382 | 108.91 | 10.89 | 0.34 | 196325.97 | `1248be19a93bc55a` |
| `9. Anita Hada v Godfather Travels.pdf` | 389211 | 51 | 63285 | 660.08 | 12.94 | 0.59 | 95874.77 | `8fb825962c7adefa` |

Aggregate: `21/21 ok`, 17974425 bytes, 1925 pages, 3272386 chars, parse wall **33528.54 ms**, weighted **17.42 ms/page**, median **13.23 ms/page**, **0.54 MB/s**, **97600.02 chars/s**.

