# bench/rigs

Hardware-tier bring-up scripts. One subdirectory per tier defined in
`testing-strategy.md`:

- `dev-laptop/` 8-core / 16GB developer machine, smoke benchmarks
- `single-node-prod/` 16-core / 32GB / NVMe SSD (added in P01)
- `cluster-3/` 3-node cluster + Redis + Postgres (added in P03)
- `cluster-10/` 10-node cluster + Redis cluster + Postgres HA (added in P07)
- `cloud/` Terraform templates for spinning up identical rigs in AWS or GCP (added in P07)

Each subdirectory has its own README with the bring-up procedure.

P00 ships the dev-laptop scaffolding. The other rigs land as the phases that
need them arrive.
