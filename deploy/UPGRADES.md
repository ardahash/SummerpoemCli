# Summerpoem upgrade policy

How new versions reach a live network without splitting it. There are two
kinds of change, handled very differently.

## 1. Networking / feature upgrades — rolling, no coordination

Changes that do **not** alter which blocks or transactions are valid: peer
logic, discovery, RPC, wallet, GUI, logging, most bug fixes. Since 0.5.6 the
wire protocol is forward-compatible (nodes ignore message fields and message
types they don't recognize), so **old and new nodes interoperate**.

Process: publish the new binary + checksums, announce it, and operators
upgrade whenever they like. No flag-day.

The only exception is a change to the **transport version** (the handshake
header). That forces a coordinated "flag-day" where all nodes must upgrade
together (old and new cleanly refuse each other). Avoid it unless the
transport layer itself must change. The 0.5.5 → 0.5.6 upgrade was the one
deliberate flag-day, done while the network was a single operator.

## 2. Consensus-rule upgrades — coordinated activation

Changes to what makes a block/transaction valid: difficulty, emission,
block size, activating the SLH-DSA vault output type, phase-2 STARK pruning,
etc. If some nodes enforce the new rule and others don't, the chain **forks**.

Two flavours:

- **Soft fork** (tightens the rules; new blocks still look valid to old
  nodes). Activates on majority hashpower; old nodes need not upgrade.
  Prefer this — segregated witnesses and the reserved address version byte
  exist so upgrades like witness pruning can ship as soft forks.
- **Hard fork** (changes/loosens rules; old nodes would reject new blocks).
  Requires ~everyone to upgrade or the network permanently splits.

Process for a consensus change:

1. **Pick an activation point** — a block height (or timestamp) in the
   future — and bake it into the release: "new rules take effect at height N."
2. **Announce with lead time** — weeks, through the releases page and node-
   operator channel, labeled clearly (e.g. "MANDATORY before height N").
3. **Measure adoption first.** Miners stamp their software version into the
   coinbase (`height(8) || "SUMP" || major.minor.patch`). Check the network's
   readiness with:
   ```
   sump node versions --blocks 500
   ```
   Only activate a hard fork once a strong majority of recent blocks are on
   the new version.
4. **Keep the beta-period safety valve** while the chain is young: the chain
   may be restarted from genesis until permanence is declared.

## Checklist for cutting a release

- [ ] Decide: networking (rolling) or consensus (coordinated)?
- [ ] Bump the workspace version.
- [ ] If consensus: set the activation height and announce it ahead of time.
- [ ] Build, run the test suite, package, publish binary + SHA-256.
- [ ] Post the genesis hash and (for consensus changes) the activation height.
- [ ] Upgrade your own seed nodes; watch `sump node versions` for adoption.
