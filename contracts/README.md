# SponsorAccount contract

EIP-7702 delegation target for opt-in EVM gas-fee sponsorship. Users delegate
their EOA to this contract; the sponsor operator can then execute 0x swaps
from the user's account and pay the gas itself. The sponsor cannot move funds
without an explicit `execute(target, value, data)` call.

- `SponsorAccount.sol` — the contract
- `test/SponsorAccount.t.sol` — Foundry security tests (6 tests)
- `foundry.toml` — Foundry config (solc 0.8.29, Cancun)

## Test

```bash
forge test
```

## Embedded bytecode

`src/eip7702.rs` embeds this contract's creation bytecode
(`SPONSOR_ACCOUNT_BYTECODE`) so `/sponsor setup` can deploy it without
compiling at runtime. Regenerate it after changing the contract:

```bash
solc --bin --optimize --evm-version cancun SponsorAccount.sol \
  | grep -A1 'Binary:' | tail -1 | tr -d '\n'
```

then paste the hex into `SPONSOR_ACCOUNT_BYTECODE` (the tail metadata hash
may differ between solc runs; bytecode is functionally identical).
