# Summerpoem: A Quantum-Resistant Peer-to-Peer Electronic Cash System

**Draft v0.1 — July 2026**
*The Summerpoem founding team*

---

**Abstract.** Every major blockchain in operation today secures its funds
with elliptic-curve signatures, which a sufficiently large quantum computer
running Shor's algorithm can forge. Summerpoem is a proof-of-work
cryptocurrency designed from genesis for the post-quantum era. It replaces
elliptic-curve cryptography with NIST-standardized lattice signatures
(ML-DSA-44), hides public keys behind SHA3-256 addresses until spend time,
and reserves a fully specified hash-based fallback scheme (SLH-DSA) that can
be activated without a redesign. Consensus is achieved by a memory-hard,
DRAM-bandwidth-bound proof-of-work that favors commodity consumer GPUs and
keeps full-node verification cheap. Transaction witnesses are segregated
from transaction identity at genesis, enabling a future upgrade in which a
single STARK proof replaces a block's signatures, reclaiming most of the
size cost of post-quantum cryptography. The design goal throughout is
simplicity: one live signature scheme, two output types, no scripting, and
no mechanism whose security cannot be plainly argued.

---

## 1. Introduction

Blockchains derive their security from two cryptographic pillars: hash
functions, which anchor proof-of-work and data integrity, and digital
signatures, which control ownership of funds. Quantum computing threatens
these pillars very unequally.

Shor's algorithm solves the discrete-logarithm problem in polynomial time.
Against ECDSA or Schnorr signatures — used by Bitcoin, Ethereum, and
virtually every other chain — this is fatal: an adversary with a large
fault-tolerant quantum computer recovers the private key from any public
key it has seen, and can then spend the associated funds. Grover's
algorithm, by contrast, offers only a quadratic speedup against hash
functions, and parallelizes poorly; a 256-bit hash retains 128 bits of
quantum preimage security, and no credible analysis projects quantum
hardware out-mining classical hardware.

The conclusion that shapes this whole design: **the quantum problem is a
signature problem.** Proof-of-work needs no rescue; the signature scheme
does. Existing chains face a painful migration — every coin must move to
new address types before public keys already exposed on-chain become
liabilities. A chain designed after the NIST post-quantum standardization
(2024) can simply start correct.

Summerpoem (currency code **SUMP**) is such a chain. Its second design
commitment, equal in weight to quantum resistance, is simplicity: every
mechanism below was chosen to be the smallest one that soundly does its
job, and several fashionable mechanisms were rejected for failing that
test.

## 2. Threat model

| Component | Primitive | Quantum attack | Consequence |
|---|---|---|---|
| Ownership | Signature scheme | Shor (breaks elliptic curves, RSA) | Fatal for EC schemes; **mitigated by construction here** |
| Proof-of-work | SHA3-256 | Grover: quadratic speedup only, poor parallelism | Negligible; 256-bit hashes retain 128-bit quantum security |
| Addresses, txids, Merkle trees | SHA3-256 | Grover (preimage/collision) | Negligible, as above |

We assume the classical security of SHA3 and the quantum-resistance of
module-lattice problems (Module-LWE/SIS). The dormant fallback path (§4.4)
removes even the lattice assumption if it is ever shaken. We explicitly do
not defend against a break of SHA3 itself; such a break invalidates every
proof-of-work chain, including this one.

## 3. Transactions

Summerpoem uses the unspent-transaction-output (UTXO) model. A transaction
consumes existing outputs and creates new ones; ownership of an output is
proven by a signature over the spending transaction.

**Structure.** A transaction is split into two parts:

- the **body**: version, inputs (each referencing a previous output),
  outputs (each an amount and a locking condition), and a locktime;
- the **witness**: the public keys and signatures authorizing each input.

The transaction id (txid) is the SHA3-256 hash of the **body only**.
Witnesses are committed separately in the block (a parallel witness Merkle
root in the header). This segregation, adopted at genesis rather than
retrofitted, has two consequences: signatures cannot malleate txids, and a
future upgrade may replace a block's entire witness section with an
aggregate validity proof (§8) without disturbing transaction identity or
history.

**Output types.** Exactly two exist at launch; there is no scripting
language.

1. `P2PKH-PQ` — pay to public-key hash: spendable by revealing a public
   key whose SHA3-256 hash matches the output, plus a valid signature.
2. `TIMELOCK` — identical, but spendable only after a stated block height.

Amounts are integers in the base unit, the **stanza**; 1 SUMP = 10⁸
stanzas.

## 4. Signatures and addresses

### 4.1 Scheme

Every input is signed with **ML-DSA-44** (FIPS 204, the scheme formerly
known as CRYSTALS-Dilithium at NIST security level 2). Public keys are
1,312 bytes; signatures are 2,420 bytes. Verification costs tens of
microseconds. Keys are multi-use; wallets behave conventionally.

ML-DSA's security rests on the hardness of Module-LWE/SIS lattice problems,
against which no efficient classical or quantum algorithm is known. It is
NIST's primary post-quantum signature standard and is being deployed across
TLS and SSH, giving it the largest cryptanalytic spotlight of any
post-quantum scheme.

The cost is size: a typical one-input, two-output transaction is ~4 KB,
roughly ten times its Bitcoin equivalent. Section 8 describes how most of
this is later reclaimed.

### 4.2 Addresses

An address is the SHA3-256 hash of a public key, truncated to 20 bytes,
prefixed with a version byte, and encoded in bech32m with human-readable
prefix `sump`. The public key therefore never appears on-chain until the
moment an output is spent. This is defense in depth: even under a
hypothetical future signature break, funds at unexposed addresses reduce
to a SHA3 preimage problem. Wallets use a fresh address per payment by
default, so exposure is limited to already-spent keys and the brief
mempool window of in-flight transactions.

### 4.3 Key derivation

Wallets derive all keys from a single mnemonic seed phrase; post-quantum
keypairs are generated deterministically from that entropy, so backup
semantics match existing industry practice.

### 4.4 The dormant fallback: SLH-DSA

The consensus rules specify, from genesis, a second output type using
**SLH-DSA-128s** (FIPS 205, SPHINCS+): a stateless hash-based signature
whose security requires nothing beyond the hash function the chain already
depends on. Its signatures are large (7,856 bytes) and signing is slow,
so it is not active at launch. It sits behind a reserved address version
byte and can be activated by soft fork in either of two futures: as an
emergency migration target should lattice cryptanalysis advance, or as an
opt-in high-assurance "vault" type for long-term cold storage. Because the
type is fully specified in advance, activation is a switch, not a redesign.

## 5. Proof-of-work: SumpHash

### 5.1 Design goals

(a) Commodity consumer hardware mines competitively; (b) ASIC and FPGA
advantage is minimized; (c) verifying a block header stays cheap enough
for any node or light client. Goal (a) is deliberately GPU-shaped rather
than CPU-shaped: CPU-mineable chains inherit botnet mining (every
compromised server competes on stolen electricity) and a cheap
cloud-rental 51% surface, and the historical record of sustained
GPU-mined chains is far deeper.

### 5.2 Algorithm

SumpHash follows the dataset/light-cache architecture proven by Ethash,
rebuilt on standardized SHA3/SHAKE:

- A **fixed 2 GiB dataset** is pseudorandomly generated each 4,096-block
  epoch (≈2.8 days) from an epoch seed. Seeds form a SHA3 hash chain
  indexed by epoch number, independent of chain content (as in Ethash),
  so miners can precompute the next epoch's dataset before the boundary
  arrives. The size never grows: 2 GiB already exceeds feasible on-die
  ASIC memory by orders of magnitude, and a growing dataset would only
  obsolete smaller consumer cards over time, as Ethereum's did.
- A **64 MiB light cache**, generated from the same seed, suffices to
  recompute any dataset item on demand.
- To attempt a nonce: derive an initial mix from SHAKE-256(header ‖ nonce);
  perform 64 sequential, data-dependent 128-byte reads from the dataset,
  folding each read into the mix with a Keccak-f-based mixing function;
  the final SHA3-256 digest of the mix must fall below the difficulty
  target.

The sequential data-dependent reads make random-access DRAM bandwidth the
binding resource. Consumer GPUs deliver more of it per dollar than any
other hardware; datacenter HBM accelerators offer ~3× the bandwidth at
~30× the price plus an enormous opportunity cost, and are uneconomical for
mining. Because the algorithm is bandwidth-bound, a hypothetical ASIC
cannot outrun DRAM physics; historical Ethash ASICs achieved only a
~2–5× power-efficiency edge, and that bounded residual risk is accepted
and further deterred by a standing commitment: **if SumpHash ASICs ship,
the algorithm is tweaked at a scheduled hard fork.**

### 5.3 Verification asymmetry

Miners need ≥4 GiB of GPU memory. Verifiers do not: a node holding the
64 MiB cache verifies a header by recomputing the 64 accessed items
directly, in milliseconds, on a CPU. Full nodes and light clients never
touch the 2 GiB dataset. Mining is deliberately harder than validating.

### 5.4 Difficulty

Difficulty retargets every block using ASERT (absolutely scheduled
exponentially rising targets): the target adjusts exponentially in the
difference between actual and scheduled cumulative block time. ASERT is
stateless given the anchor block, provably stable, and responsive to the
large hashrate swings typical of a young chain.

## 6. Network

Nodes gossip transactions and blocks over TCP. Peer connections are
encrypted with a post-quantum key exchange (**ML-KEM**, FIPS 203) — a
chain should not carry quantum-resistant signatures over
quantum-breakable transport. Standard mempool admission (signature
validity, no double-spends, fee floor) and compact-block relay apply; the
longest-cumulative-work chain is canonical.

New nodes download and verify headers first (cheap, per §5.3), then block
bodies. Nodes may prune spent transaction data; witness data of
sufficiently buried blocks may additionally be discarded under the rules
of §8.

## 7. Consensus parameters and emission

| Parameter | Value |
|---|---|
| Block interval | 60 seconds (target) |
| Block size limit | 4 MB (≈1,000 typical transactions, ≈16 tx/s) |
| Difficulty adjustment | ASERT, per block |
| Coinbase maturity | 100 blocks |
| Total supply | 42,000,000 SUMP (hard cap) |
| Premine / dev fund | none — fair launch |

**Emission.** Issuance follows a smooth geometric decay — a continuous
analogue of halvings, with no cliff events — defined entirely in integer
arithmetic with an exact rational base (no transcendental constants in
consensus code). With M = 3,033,415 and C = 42,000,000 SUMP, the block-h
subsidy is

  R(h) = R₀ × ((M−1)/M)ʰ,  R₀ = ⌊C/M⌋ = 1,384,578,109 stanzas ≈ 13.8458 SUMP,

evaluated in Q96 fixed point by square-and-multiply. The half-life is
M·ln2 ≈ 2,102,573 blocks ≈ 4 years: the reward falls to half of any given
level every four years, half of all SUMP exists after year 4, three
quarters after year 8. Because ∑R(h) = R₀·M and all rounding is downward,
total issuance is ≤ C by construction — the cap needs no separate
enforcement rule. Miners additionally collect transaction fees, which
become the dominant incentive as issuance decays.

## 8. Scaling path: STARK witness compression

Post-quantum signatures are the dominant cost of this design: ~2.4 MB of
every full 4 MB block is witness data. Summerpoem's planned second phase
removes most of that cost using zero-knowledge proofs — with the critical
constraint that only **hash-based proof systems (STARKs)** qualify, since
pairing-based SNARKs rest on elliptic curves and fall to Shor exactly as
ECDSA does.

The upgrade activates in the least invasive order:

1. **Proof-carrying pruning.** Anyone may publish a STARK proving "every
   witness in block B verifies against its body." Blocks remain valid
   with raw witnesses; a valid proof simply entitles nodes to discard
   B's witness data (~10–20× weight reduction for storage and initial
   sync). No burden falls on block producers, and no consensus rules
   change for miners.
2. **Consensus hardening (only if needed).** If throughput demands it,
   a later fork can admit blocks that carry the aggregate proof instead
   of raw witnesses. This step would likely pair with hash-based
   one-time signatures (WOTS+) behind the reserved address version byte,
   since hash verification is dramatically cheaper than lattice
   verification inside a STARK circuit.

Three genesis rules make this path cheap: witness segregation (§3),
the reserved address version byte (§4.4), and a witness-prunability rule
specified before any prover exists. Nothing about phase 2 is required
for the chain's security — aggregation is a scaling feature; the
quantum defense is complete without it.

## 9. Launch policy

Summerpoem launches fairly: no premine, no developer allocation, no
early-access period. The founding team mines the early chain under the
same rules as any participant. The genesis subsidy itself is paid to the
all-zero key hash — provably unspendable by anyone, founders included.

A reference implementation in Rust — full consensus validation, wallet,
genesis builder, and CPU reference miner, with regression and integration
test coverage — accompanies this paper and serves as the executable
specification until a formal one is written.

There is no public testnet phase. With nothing at stake at genesis, the
early mainnet serves that role — as Bitcoin's did. Two commitments make
this safe: the launch is publicly declared **provisional**, meaning the
chain may be restarted from genesis until the team explicitly announces
permanence; and reproducible local networks (regtest) ship in the node
software for development and integration testing. Permanence will be
declared only after the network has demonstrated stability under real
conditions.

## 10. Conclusion

Summerpoem applies a simple observation — that quantum computing breaks
blockchain signatures, not blockchain hashing — and builds the smallest
sound system around it: standardized lattice signatures behind hashed
addresses, a hash-based fallback specified in advance, a memory-hard
proof-of-work that keeps mining on consumer hardware and validation on
any hardware, and a witness layout that lets tomorrow's hash-based proof
systems reclaim today's signature overhead. Nothing here is novel
cryptography, deliberately: every primitive is a published standard with
years of public analysis, composed conservatively. The result is a chain
that needs no quantum migration, because it never made the assumption
that quantum computers break.

---

## References

1. NIST FIPS 204, *Module-Lattice-Based Digital Signature Standard*
   (ML-DSA), 2024.
2. NIST FIPS 205, *Stateless Hash-Based Digital Signature Standard*
   (SLH-DSA), 2024.
3. NIST FIPS 203, *Module-Lattice-Based Key-Encapsulation Mechanism
   Standard* (ML-KEM), 2024.
4. NIST FIPS 202, *SHA-3 Standard: Permutation-Based Hash and
   Extendable-Output Functions*, 2015.
5. S. Nakamoto, *Bitcoin: A Peer-to-Peer Electronic Cash System*, 2008.
6. P. Shor, *Polynomial-Time Algorithms for Prime Factorization and
   Discrete Logarithms on a Quantum Computer*, 1997.
7. L. Grover, *A Fast Quantum Mechanical Algorithm for Database Search*,
   1996.
8. Ethereum Foundation, *Ethash* specification (dataset/light-cache
   proof-of-work architecture), 2015.
9. M. Toomim, J. Sechet, *ASERT: Absolutely Scheduled Exponentially
   Rising Targets* (aserti3-2d difficulty algorithm), 2020.
10. E. Ben-Sasson et al., *Scalable, transparent, and post-quantum secure
    computational integrity* (STARKs), 2018.
