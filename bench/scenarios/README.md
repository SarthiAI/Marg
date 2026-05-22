# bench/scenarios

One file per benchmark scenario. Naming convention:
`<id>-<short-name>.{k6.js,sh}` per `testing-strategy.md`.

Scenarios are introduced phase by phase:

- P01: L01, L02, L04, L05, T01, T02, B01
- P02: C01
- P03: T03, C04
- P04: S01
- P07: T04, T05, C02, C03, C05, C06, B02, B03, S02, R01 to R05
- P08: L03, T06, K01, K03, K04, K05, K06, S03
- P09: T07, K02

P00 ships this directory empty.
