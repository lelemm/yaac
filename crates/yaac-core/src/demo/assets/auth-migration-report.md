# Auth migration report

**Status:** users-service and billing-service are on the new auth module; search-service is rebuilding its index.

| Service | State | Tests |
|---|---|---|
| users-service | merged | 14 passed |
| billing-service | merged | 14 passed, 1 skipped (`test_refund_race`) |
| search-service | rebuilding the index | — |

## Next

1. Finish the search index rebuild.
2. Remove `legacy/auth` once nothing imports it.
