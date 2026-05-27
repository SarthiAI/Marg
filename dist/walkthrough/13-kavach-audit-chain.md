# 13 - Kavach audit chain end-to-end

## Goal

Live chain verifies. Disk-resident JSONL file verifies. Byte-flipping the
JSONL file produces `verified: false` with the offending index reported.

## Setup

Marg running. At least 20 requests have flowed through prior scenarios so
the chain has real content.

## Steps

```walkthrough
# 1. Live chain verifies
ADMIN POST /admin/audit/verify '{}' 200 jq '.verified == true' jq '.count > 0'

# 2. Force a flush (or wait `audit_flush_seconds`), then verify the on-disk file
SLEEP $audit_flush_seconds + 2
ASSERT file present under [kavach].audit_export_path matching audit-*.jsonl
ADMIN POST /admin/audit/verify '{"path":"<that file>"}' 200 jq '.verified == true'

# 3. Byte-flip the file, re-verify
EDIT <that file> flip 1 byte at offset 32
ADMIN POST /admin/audit/verify '{"path":"<that file>"}' 200 jq '.verified == false' jq '.error'

# 4. Restore the byte and re-verify (should pass again)
EDIT <that file> restore
ADMIN POST /admin/audit/verify '{"path":"<that file>"}' 200 jq '.verified == true'

# 5. Cross-restart parking (ADR-016): restart marg, verify the live chain restarts at index 0
RESTART marg
ADMIN POST /admin/audit/verify '{}' 200 jq '.count == 0'
ASSERT prior JSONL files still verify independently
```

## Expected

- Live chain verify always works.
- File-level verify follows the live state.
- A flipped byte produces a documented failure with `error` describing
  which signature or hash mismatched.
- Restart starts a fresh in-memory chain (ADR-016).

## Cleanup

Restore any flipped bytes. No persistent state changes.
