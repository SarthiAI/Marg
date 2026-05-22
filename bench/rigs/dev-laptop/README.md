# dev-laptop rig

Smallest benchmark tier. 8-core / 16GB developer machine. Used for smoke
benchmarks and single-instance baseline numbers during development.

## Scripts

- `setup.sh` checks the local environment (k6, Marg binary, ports) and prints
  what is missing.
- `run.sh` runs the dev-laptop subset of the active scenarios against a local
  Marg instance and writes results into the rig-local results directory.

P00 ships these as scaffolding; they are filled in P01 once the first real
scenarios (L01, L02, T01, T02) exist.

## Usage

```bash
./setup.sh
./run.sh
```
