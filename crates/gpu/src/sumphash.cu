// SumpHash v1 CUDA kernel. Must stay bit-identical to the CPU reference in
// crates/pow/src/lib.rs (PowContext::compute). GPUs are little-endian, and
// Keccak lanes are little-endian, so the sponge state can be addressed as a
// byte array directly.

typedef unsigned char u8;
typedef unsigned int u32;
typedef unsigned long long u64;

#define ITEM 64
#define PAGE 128
#define RATE 136          // SHA3-256 and SHAKE-256 both have 136-byte rate

__device__ __constant__ u64 RC[24] = {
    0x0000000000000001ULL, 0x0000000000008082ULL, 0x800000000000808aULL,
    0x8000000080008000ULL, 0x000000000000808bULL, 0x0000000080000001ULL,
    0x8000000080008081ULL, 0x8000000000008009ULL, 0x000000000000008aULL,
    0x0000000000000088ULL, 0x0000000080008009ULL, 0x000000008000000aULL,
    0x000000008000808bULL, 0x800000000000008bULL, 0x8000000000008089ULL,
    0x8000000000008003ULL, 0x8000000000008002ULL, 0x8000000000000080ULL,
    0x000000000000800aULL, 0x800000008000000aULL, 0x8000000080008081ULL,
    0x8000000000008080ULL, 0x0000000080000001ULL, 0x8000000080008008ULL};

__device__ __forceinline__ u64 rotl64(u64 x, int n) {
    return (x << n) | (x >> (64 - n));
}

__device__ void keccakf(u64 st[25]) {
    const int rho[24] = {1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14,
                         27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44};
    const int pi[24] = {10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4,
                        15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1};
    for (int round = 0; round < 24; round++) {
        u64 bc[5];
        for (int i = 0; i < 5; i++)
            bc[i] = st[i] ^ st[i + 5] ^ st[i + 10] ^ st[i + 15] ^ st[i + 20];
        for (int i = 0; i < 5; i++) {
            u64 t = bc[(i + 4) % 5] ^ rotl64(bc[(i + 1) % 5], 1);
            for (int j = 0; j < 25; j += 5)
                st[j + i] ^= t;
        }
        u64 t = st[1];
        for (int i = 0; i < 24; i++) {
            int j = pi[i];
            u64 tmp = st[j];
            st[j] = rotl64(t, rho[i]);
            t = tmp;
        }
        for (int j = 0; j < 25; j += 5) {
            u64 tt[5];
            for (int i = 0; i < 5; i++)
                tt[i] = st[j + i];
            for (int i = 0; i < 5; i++)
                st[j + i] = tt[i] ^ ((~tt[(i + 1) % 5]) & tt[(i + 2) % 5]);
        }
        st[0] ^= RC[round];
    }
}

// Single-block sponge: absorb msg (< RATE bytes), pad, permute, copy out bytes.
__device__ void keccak_block(const u8 *msg, int len, u8 domain, u8 *out,
                             int outlen) {
    u64 st[25];
    for (int i = 0; i < 25; i++)
        st[i] = 0;
    u8 *sb = (u8 *)st;
    for (int i = 0; i < len; i++)
        sb[i] ^= msg[i];
    sb[len] ^= domain;
    sb[RATE - 1] ^= 0x80;
    keccakf(st);
    for (int i = 0; i < outlen; i++)
        out[i] = sb[i];
}

__device__ __forceinline__ u32 fnv(u32 a, u32 b) {
    return (a * 0x01000193u) ^ b;
}

__device__ __forceinline__ u32 le32(const u8 *p) {
    return (u32)p[0] | ((u32)p[1] << 8) | ((u32)p[2] << 16) | ((u32)p[3] << 24);
}

__device__ void sumphash(const u8 *dataset, u32 pages, u32 accesses,
                         const u8 *pow_message, u64 nonce, u8 out[32]) {
    // SHAKE-256("sump/mixseed" || pow_message || nonce_le) -> 64 bytes
    u8 buf[52];
    const char tag[12] = {'s', 'u', 'm', 'p', '/', 'm',
                          'i', 'x', 's', 'e', 'e', 'd'};
    for (int i = 0; i < 12; i++)
        buf[i] = (u8)tag[i];
    for (int i = 0; i < 32; i++)
        buf[12 + i] = pow_message[i];
    for (int i = 0; i < 8; i++)
        buf[44 + i] = (u8)(nonce >> (8 * i));
    u8 seed64[64];
    keccak_block(buf, 52, 0x1f, seed64, 64);

    u8 mix[PAGE];
    for (int i = 0; i < ITEM; i++) {
        mix[i] = seed64[i];
        mix[ITEM + i] = seed64[i];
    }
    u32 s0 = le32(seed64);
    for (u32 a = 0; a < accesses; a++) {
        u32 lane = le32(&mix[(a % 32) * 4]);
        u32 idx = fnv(a ^ s0, lane) % pages;
        const u8 *page = dataset + (size_t)idx * PAGE;
        for (int k = 0; k < PAGE / 4; k++) {
            u32 m = le32(&mix[4 * k]);
            u32 o = le32(&page[4 * k]);
            u32 v = fnv(m, o);
            mix[4 * k + 0] = (u8)v;
            mix[4 * k + 1] = (u8)(v >> 8);
            mix[4 * k + 2] = (u8)(v >> 16);
            mix[4 * k + 3] = (u8)(v >> 24);
        }
    }

    // compress 128 -> 32
    u8 cmix[32];
    for (int k = 0; k < 8; k++) {
        u32 m0 = le32(&mix[4 * (4 * k + 0)]);
        u32 m1 = le32(&mix[4 * (4 * k + 1)]);
        u32 m2 = le32(&mix[4 * (4 * k + 2)]);
        u32 m3 = le32(&mix[4 * (4 * k + 3)]);
        u32 c = fnv(fnv(fnv(m0, m1), m2), m3);
        cmix[4 * k + 0] = (u8)c;
        cmix[4 * k + 1] = (u8)(c >> 8);
        cmix[4 * k + 2] = (u8)(c >> 16);
        cmix[4 * k + 3] = (u8)(c >> 24);
    }

    // SHA3-256("sump/pow/v1" || seed64 || cmix) -> 32 bytes
    u8 fin[107];
    const char ptag[11] = {'s', 'u', 'm', 'p', '/', 'p', 'o', 'w', '/', 'v', '1'};
    for (int i = 0; i < 11; i++)
        fin[i] = (u8)ptag[i];
    for (int i = 0; i < 64; i++)
        fin[11 + i] = seed64[i];
    for (int i = 0; i < 32; i++)
        fin[75 + i] = cmix[i];
    keccak_block(fin, 107, 0x06, out, 32);
}

// hash <= target, both big-endian 256-bit (hash[0] most significant)
__device__ bool meets(const u8 *hash, const u8 *target) {
    for (int i = 0; i < 32; i++) {
        if (hash[i] < target[i])
            return true;
        if (hash[i] > target[i])
            return false;
    }
    return true;
}

extern "C" __global__ void sumphash_search(const u8 *dataset, u32 pages,
                                           u32 accesses, const u8 *pow_message,
                                           u64 start_nonce, u32 n,
                                           const u8 *target, u64 *found) {
    u32 tid = blockIdx.x * blockDim.x + threadIdx.x;
    if (tid >= n)
        return;
    u64 nonce = start_nonce + tid;
    u8 hash[32];
    sumphash(dataset, pages, accesses, pow_message, nonce, hash);
    if (meets(hash, target))
        atomicMin(found, nonce);
}

extern "C" __global__ void sumphash_hash(const u8 *dataset, u32 pages,
                                         u32 accesses, const u8 *pow_message,
                                         u64 start_nonce, u32 n, u8 *out) {
    u32 tid = blockIdx.x * blockDim.x + threadIdx.x;
    if (tid >= n)
        return;
    sumphash(dataset, pages, accesses, pow_message, start_nonce + tid,
             &out[(size_t)tid * 32]);
}
