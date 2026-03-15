# Transfer Artifacts

This folder is for moving the repository to a new development machine.

## Files created by the transfer script

1. `WirelessExplorer.bundle`
   - full git history bundle
2. `WirelessExplorer-working-tree.tar.gz`
   - source tree snapshot without `.git/`, `target/`, screenshots, or runtime output
3. `SHA256SUMS`
   - checksums for the generated artifacts

## Generate artifacts

From the repo root:

```bash
bash continuity/transfer/make_transfer_artifacts.sh
```

## Restore from the git bundle

```bash
git clone WirelessExplorer.bundle WirelessExplorer
cd WirelessExplorer
bash continuity/bootstrap_ubuntu.sh
```

## Restore from the working-tree tarball

```bash
mkdir -p WirelessExplorer
cd WirelessExplorer
tar -xzf ../WirelessExplorer-working-tree.tar.gz
bash continuity/bootstrap_ubuntu.sh
```
