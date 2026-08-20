# Supreme Court corpus constrained benchmark — 2026-07-21

The three runs use the same cached 21 PDFs, extraction options, and release binary.
Fetch/cache time is separate from parse wall time in every source report.

## Aggregate comparison

| profile | scheduler / workers | parse wall ms | weighted ms/page | median ms/page | MB/s | chars/s | throughput vs full |
|---|---|---:|---:|---:|---:|---:|---:|
| Full-core | normal / 8 | 40254.74 | 20.91 | 21.07 | 0.45 | 81268.55 | **1.000x** |
| 2-core | normal / 2 | 46060.48 | 23.93 | 23.12 | 0.39 | 71024.97 | **0.874x** |
| Efficiency-core QoS | taskpolicy -c background / 8 | 356524.55 | 185.21 | 241.60 | 0.05 | 9175.93 | **0.113x** |

Ratios use aggregate MB/s (the same ratios result from weighted ms/page and chars/s).
The 2-core run is a real parser pool cap (RAYON_NUM_THREADS=2). The efficiency-core run uses macOS taskpolicy -c background; macOS does not expose strict per-process E-core affinity, so this is a QoS scheduling approximation, not a claim of hard pinning.

## What this says about a 2-vCPU cx23-class box

The 2-core profile is the useful parallelism analogue: it processes this corpus in 0.874x of full-core throughput, or 46.06 s of parser-only wall on this M1 Pro. That is not an absolute cloud prediction—the cloud CPU is x86 and shared—but it sets the honest local expectation: parser work is tens of seconds for these 21 PDFs, while the reported 405 s backend ingestion figure also contains object-store, job-runner, and archive overhead.

## Per-document normalized metrics

| filename | bytes | pages | chars | full ms/page | full MB/s | full chars/s | 2-core ms/page | 2-core MB/s | 2-core chars/s | E-core ms/page | E-core MB/s | E-core chars/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1. Energy Watchdog v CERC.pdf | 405704 | 65 | 113710 | 23.58 | 0.26 | 74174.75 | 25.82 | 0.24 | 67764.51 | 208.41 | 0.03 | 8393.96 |
| 1.pdf | 7310114 | 69 | 111329 | 19.09 | 5.55 | 84518.06 | 18.11 | 5.85 | 89075.80 | 173.90 | 0.61 | 9278.23 |
| 10. Parswanath Saha v Bandhana Modak.pdf | 307312 | 31 | 40977 | 21.07 | 0.47 | 62746.14 | 23.12 | 0.43 | 57164.89 | 227.86 | 0.04 | 5801.09 |
| 11. Roxann v Arun Sharma.pdf | 274193 | 21 | 30190 | 26.01 | 0.50 | 55271.93 | 30.52 | 0.43 | 47110.29 | 248.46 | 0.05 | 5786.17 |
| 12. CCI v Kerala Film Exhibitors Federation & Ors.pdf | 772345 | 68 | 102128 | 16.92 | 0.67 | 88760.96 | 19.71 | 0.58 | 76180.55 | 146.80 | 0.08 | 10231.03 |
| 13. Kumari Shrilekha Vidyarthi v State of UP.pdf | 105975 | 26 | 89910 | 43.21 | 0.09 | 80021.33 | 50.37 | 0.08 | 68656.71 | 351.00 | 0.01 | 9851.93 |
| 14. Tomaso Bruno v State of UP.pdf | 297154 | 31 | 41743 | 19.52 | 0.49 | 68983.85 | 21.50 | 0.45 | 62631.24 | 135.33 | 0.07 | 9949.90 |
| 15. MC Mehta v UoI.pdf | 82952 | 20 | 69533 | 44.27 | 0.09 | 78533.11 | 50.57 | 0.08 | 68744.53 | 345.63 | 0.01 | 10059.02 |
| 16. BWSSB v A Rajappa.pdf | 329348 | 81 | 258756 | 37.69 | 0.11 | 84765.59 | 44.35 | 0.09 | 72024.78 | 304.32 | 0.01 | 10497.35 |
| 17. Sunil B Naik v Geowave Commander.pdf | 225921 | 57 | 81712 | 18.79 | 0.21 | 76290.99 | 20.43 | 0.19 | 70172.23 | 164.82 | 0.02 | 8697.43 |
| 18. Mardia Chemicals v UoI.pdf | 150263 | 40 | 116273 | 34.74 | 0.11 | 83672.37 | 41.53 | 0.09 | 69994.65 | 313.76 | 0.01 | 9264.53 |
| 19. CBSE v Aditya Bandopadyay.pdf | 270770 | 54 | 85339 | 18.33 | 0.27 | 86227.30 | 20.78 | 0.24 | 76054.33 | 241.60 | 0.02 | 6541.23 |
| 2. Devas v Antrix.pdf | 671248 | 134 | 152464 | 13.09 | 0.38 | 86890.11 | 16.91 | 0.30 | 67279.26 | 143.04 | 0.04 | 7954.46 |
| 20 RC Cooper v UoI.pdf | 430257 | 108 | 370454 | 37.99 | 0.10 | 90284.67 | 41.01 | 0.10 | 83637.80 | 402.51 | 0.01 | 8521.83 |
| 3. Gayatri Balasamy v ISG.pdf | 1191356 | 190 | 344502 | 23.42 | 0.27 | 77427.24 | 29.76 | 0.21 | 60925.70 | 330.03 | 0.02 | 5493.94 |
| 4. Har Narain v Mam Chand.pdf | 163756 | 22 | 19660 | 17.97 | 0.41 | 49733.30 | 18.11 | 0.41 | 49333.56 | 255.33 | 0.03 | 3499.96 |
| 5. Vodafone v UoI.pdf | 1524729 | 274 | 304719 | 12.71 | 0.44 | 87483.67 | 13.59 | 0.41 | 81835.78 | 87.17 | 0.06 | 12757.67 |
| 6. Gujarat Bottling v Coca Cola.pdf | 104232 | 26 | 87252 | 49.50 | 0.08 | 67796.14 | 45.92 | 0.09 | 73079.59 | 312.63 | 0.01 | 10734.39 |
| 7. Puttaswamy v UoI.pdf | 2930333 | 547 | 766148 | 16.62 | 0.32 | 84280.69 | 19.41 | 0.28 | 72167.87 | 103.66 | 0.05 | 13511.83 |
| 8. Vishaka v State of Rajasthan.pdf | 37252 | 10 | 21377 | 46.58 | 0.08 | 45888.32 | 52.53 | 0.07 | 40697.97 | 295.12 | 0.01 | 7243.39 |
| 9. Anita Hada v Godfather Travels.pdf | 389211 | 51 | 63268 | 17.82 | 0.43 | 69629.20 | 21.55 | 0.35 | 57554.85 | 121.81 | 0.06 | 10184.38 |
