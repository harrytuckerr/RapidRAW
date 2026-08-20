# NOTICE

This is a fork of [RapidRAW](https://github.com/CyberTimon/RapidRAW), (c) Timon Kach, licensed under AGPL-3.0.

## What this fork adds

An Adobe DNG Camera Profile (DCP) pipeline and Cobalt Image profile support, enabling camera-profile-based rendering inside RapidRAW on desktop and Android. The feature adds a first-class "profile slot" to the editing pipeline (the concept Lightroom/ACR exposes as `Adobe Color` / `Camera Standard` / a Cobalt profile), holding a base DCP plus an optional Cobalt Look and an Amount, applied before all existing adjustments.

## Specifications and trademarks

- **DNG / DCP** is an Adobe specification. Adobe is a trademark of Adobe Inc. No Adobe code or assets are redistributed in this repository.
- **Cobalt Image** is a trademark of its owner. No Cobalt-Image code, profiles, look tables, or other assets are redistributed in this repository, in source, in tests, in fixtures, or in built binaries. Users install their own lawfully-purchased profile files.
- This fork implements **read-only interoperability** with profile files the user already owns. It does not author, convert, export, or redistribute profiles, and it does not circumvent any licence check. A profile's `ProfileEmbedPolicy` (e.g. `EmbedNever`) is honoured on export.

## Licensing

This fork remains licensed under AGPL-3.0, identical to upstream. See `LICENSE`. AGPL obligations apply to any network-served deployment.

## Upstream attribution

Upstream RapidRAW is (c) Timon Kach, AGPL-3.0: https://github.com/CyberTimon/RapidRAW
