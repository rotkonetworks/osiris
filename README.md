# Osiris: a trading bot for the Penumbra shielded DEX 💹

[Osiris](https://en.wikipedia.org/wiki/OSIRIS-REx) is used for supplying liquidity to Penumbra testnets
by replicating market prices from the Binance public API. Osiris also serves as an example for how
a simple trading bot can be built against the Penumbra DEX.

This bot is the Alameda Research of Penumbra. It performs no risk management, portfolio rebalancing, or any other
best practices and only submits trades. Please use it only as a reference.

It does not duplicate command-line wallet management; rather, it shares a wallet by default with the
location of the wallet managed by the `pcli` command line Penumbra wallet. To set up Osiris, first
create a wallet with `pcli`, then send some tokens to that wallet on the test network. Then, you can
run Osiris:

## Obtaining dependencies

You must clone the [penumbra repo](https://github.com/penumbra-zone/penumbra)
side-by-side with the Osiris repo, so that the dependencies are available
as a relative path. This is a temporary workaround to support Git LFS
in the Penumbra dependencies.
See [GH29](https://github.com/penumbra-zone/galileo/issues/29) for details.

## Running it

```bash
RUST_LOG=osiris=debug cargo run --release serve USDT ETH OSMO BTC
```

This will monitor the [Binance websockets API](https://developers.binance.com/docs/binance-trading-api/websocket_api) for
current market prices between all pairings of the assets provided on the CLI.

Based on current market prices, Osiris will use as much of its available liquidity as possible to provide positions
replicating Binance's market conditions within the Penumbra shielded DEX, using the positions' spread parameter to
represent the bid/ask spread from the market feed.

On first synchronization, the wallet must be caught up to speed with the state of the chain, which
can take some time; the `info`-level log output will inform you when the bot is ready.

A variety of options are available, including adjusting replication timing, and changing which node to
connect to (the default is the hosted Penumbra default testnet). Use the `--help` option for more details.

## Re-deploying after a testnet release

During deploy of a new testnet, Osiris will automatically be restarted, but
it won't be using a new image built from the latest code. Sometimes that's OK,
but we aim to keep the deployments in sync, so the dependencies match.
Perform these steps manually after deploying a new testnet:

1. Rebuild the Osiris container via [GHA](https://github.com/penumbra-zone/osiris/actions),
   passing in the Penumbra tag version to build from, e.g. `v0.58.0`.
2. Wait for the container build to complete, then run:
   `kubectl set image deployments -l app.kubernetes.io/instance=osiris-testnet osiris=penumbra-v0.58.0`
   substituting the correct version in the tag name.

Eventually we should automate these steps so they're performed automatically as part of a release.

# Bouncing deployments

Restarting the Osiris service will cause the deployment to pull for a new container image.
If a newer container image exists in the remote repository (`ghcr.io/penumbra-zone/osiris`),
that image will be used. You must manually build a new image via the [GHA setup](https://github.com/penumbra-zone/osiris/actions).

```
# For the preview deployment:
kubectl rollout restart deployment osiris-preview

# For the testnet deployment:
kubectl rollout restart deployment osiris-testnet

# View logs for Osiris at any time with:
kubectl logs -f $(kubectl get pods -o name -l app.kubernetes.io/instance=osiris-preview)
```

# License

By contributing to Osiris you agree that your contributions will be licensed
under the terms of both the [LICENSE-Apache](LICENSE-Apache) and the
[LICENSE-MIT](LICENSE-MIT) files in the root of this source tree.

If you're using Osiris you are free to choose one of the provided licenses.

`SPDX-License-Identifier: MIT OR Apache-2.0`
