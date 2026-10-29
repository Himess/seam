# Contributing to Seam

Seam is consensus code that other contracts rely on, so changes are held to the rules below. They follow
librustzcash's [styleguides](https://github.com/zcash/librustzcash/blob/main/CONTRIBUTING.md#styleguides), adapted to
a Foundry project. Questions and ideas are welcome as issues. Security reports go through [SECURITY.md](SECURITY.md),
never a public issue.

## Build and test

With Foundry 1.8.5 and Python 3 (standard library only):

```sh
git clone --recursive https://github.com/Himess/seam
cd seam/contracts
forge build
forge test
forge fmt --check src/interfaces src/examples test/examples test/deployments
```

## Changes

- **One change per pull request.** Split larger work into stacked pull requests: each one branches from, and targets,
  the one before it.
- **A test for every change.** A bug fix starts with a commit that adds a failing test, then the commit that fixes it.
- **Consensus code** (`contracts/src/relay/`: the header rules, Equihash, the difficulty adjustment, the time rules):
  - a failing test first, on real headers wherever they exist;
  - the Python reference (`contracts/test/relay/tools/zcash_header_check.py`) and the Equihash corpus
    (`contracts/test/relay/fixtures/equihash_corpus.json`) kept in agreement with the Solidity;
  - the Zebra source or the ZIP each rule follows, cited next to it.
- **Network parameters** (`contracts/config/relay-*.json`): each value with its source, and tests on real headers.
- **Formatting:** `forge fmt` on new Solidity.
- **Changelog:** a change users can see (an interface, a deployment, a behaviour) gets an entry in `CHANGELOG.md`, in
  the same commit, in a section for the next release.
- **English** for code, comments, docs, commit messages and issues.

## Branch history

- Commits are discrete semantic changes. No "work in progress" commits: fold fixes and review changes into the commit
  they belong to ([git revise](https://github.com/mystor/git-revise) helps).
- Pull requests are merged with merge commits. No squash or rebase merges, except for a pull request without a clean
  history.
- If the target branch moves and causes a conflict, rebase the pull request on it.
- Once someone is reviewing a pull request, add commits instead of force-pushing, unless they agree to a rebase.

## Commit messages

- A short title, under about 120 characters, with an optional scope: `feat(relay):`, `test(examples):`, `docs:`.
- A body that says why the change is made, unless it is obvious.
- A `Co-Authored-By:` line for everyone responsible for the commit's content.
