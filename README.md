# introspectardio

A tiny proof-of-concept showing how to eliminate some of the CPI overhead on token transfers in solana programs using transaction introspection.

Because transfers will soon drop from ~5000 CUs to ~70 CUs (thanks to ptoken), the remaining ~1000 CU CPI cost becomes the dominant overhead. This demo implements a super simple, fixed-price, one-sided “market” that reads the user’s incoming SPL token transfer directly from the previous instruction in the same transaction to eliminate one CPI.

The flow is roughly:

1. User optimistically transfers token A into the pool vault.

2. The program introspects the previous instruction to read the amount received.

3. It computes an output amount and transfers token B from the pool vault—without loading the user as a signer and without routing through the token program again.

## WARNING: This is not a production-ready AMM

It’s unaudited, with a fixed price, one-sided, with no slippage checks.

Other implementors should be aware of the main footgun: a program doing multiple CPIs into a swap program could cause the same transfer to be “seen” twice.

proceed with caution. use at your own risk. i am not responsible for any loss of funds. if you do not know who robert chen is, now would be a good time to learn who that is.
