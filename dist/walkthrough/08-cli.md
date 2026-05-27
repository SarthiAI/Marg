# 08 - CLI

## Goal

Every `marg` subcommand prints the documented shape and exits 0 (or 1 on
documented errors).

## Steps

```walkthrough
CLI marg version --verbose                  prints marg, kavach_core, kavach_pq, kavach_redis
CLI marg db migrate --check                  prints applied + pending counts
CLI marg keys create --principal-id wt-cli --kind user --team eng   prints token + key id once
CLI marg keys list                           prints at least one row
CLI marg keys revoke <id>                    prints {"revoked": true}
CLI marg budget show <key_id>                prints daily_usd, daily_used_usd, rpm
CLI marg budget set <key_id> --daily 2 --rpm 120   prints {"updated": true}
CLI marg log tail --limit 5                  prints up to 5 rows with attempts column
CLI marg admin bootstrap                     prints existing token path + permissions (0600)
CLI marg admin tokens list                   prints at least one active row
CLI marg admin tokens revoke <id>            prints {"revoked": true}
CLI marg policy audit --since 24h            prints recent audit entries (may be 0 on a fresh box)
```

## Expected

Every subcommand prints valid JSON (or the documented text form) and exits
0. Token-printing commands emit the token exactly once.

## Cleanup

Drop the test key + rotation token at the end.
