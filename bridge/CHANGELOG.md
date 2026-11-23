# Changelog

## [0.4.0](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.3.0...faderz-bridge-v0.4.0) (2026-11-23)


### ⚠ BREAKING CHANGES

* **strip:** the state model's Comp requires mix, and DERIVATION_VERSION is 13 with samplez_replica::PROTOCOL 7.

### Features

* **strip:** add a parallel mix to every compressor ([a277afc](https://github.com/danielclasen/faderz/commit/a277afccd7021bae75c6ace295e81f9f80f850d8))


## [0.3.0](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.2.1...faderz-bridge-v0.3.0) (2026-11-22)


### Bug Fixes

* **deps:** take url 2.5.9 for RUSTSEC-2026-0592 ([65e72e8](https://github.com/danielclasen/faderz/commit/65e72e88a4504dc656a30d72231c7e0f198014ae))

## [0.2.1](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.2.0...faderz-bridge-v0.2.1) (2026-11-19)


### Features

* **mix:** pan a mono channel in the field of a 5.1 or 7.1 bus ([#104](https://github.com/danielclasen/faderz/issues/104)) ([e44204e](https://github.com/danielclasen/faderz/commit/e44204eb3928aa3cc806e1b69d1db5b785d30d96))

## [0.2.0](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.1.4...faderz-bridge-v0.2.0) (2026-11-18)


### ⚠ BREAKING CHANGES

* **audio:** meter frames are format 4; a decoder of format 3 drops them.

### Features

* **audio:** duck targets under their groups' triggers ([2343d4d](https://github.com/danielclasen/faderz/commit/2343d4d789713eb29f3c48e353152d0722469847))
* **bridge:** light duck keys while their group ducks ([2343d4d](https://github.com/danielclasen/faderz/commit/2343d4d789713eb29f3c48e353152d0722469847))
* **web:** show duck key level and gain reduction from meter frames ([2343d4d](https://github.com/danielclasen/faderz/commit/2343d4d789713eb29f3c48e353152d0722469847))

## [0.1.4](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.1.3...faderz-bridge-v0.1.4) (2026-11-18)


### Bug Fixes

* **bridge:** put the renewed token in use before it is stored ([#60](https://github.com/danielclasen/faderz/issues/60)) ([deb9aaf](https://github.com/danielclasen/faderz/commit/deb9aaf68f893468a41ae135ab4ca305acd4ce1d))

## [0.1.3](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.1.2...faderz-bridge-v0.1.3) (2026-11-18)


### Features

* **bridge:** drive duck groups from function keys ([#56](https://github.com/danielclasen/faderz/issues/56)) ([421f212](https://github.com/danielclasen/faderz/commit/421f212a605dcb763b93704a10b691b1440db180))


### Bug Fixes

* **bridge:** wait out the tap time before holding a bypassed duck ([#59](https://github.com/danielclasen/faderz/issues/59)) ([d14079e](https://github.com/danielclasen/faderz/commit/d14079ee9f4253bca8bd45329b917afa778b9880))

## [0.1.2](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.1.1...faderz-bridge-v0.1.2) (2026-11-18)


### Features

* **bridge:** add an RTP-MIDI transport ([#51](https://github.com/danielclasen/faderz/issues/51)) ([415dedf](https://github.com/danielclasen/faderz/commit/415dedfc88b3ab06e707214a31c4d5309aab9172))

## [0.1.1](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.1.0...faderz-bridge-v0.1.1) (2026-11-17)


### Bug Fixes

* **bridge:** send PKCE with the device authorization grant ([#40](https://github.com/danielclasen/faderz/issues/40)) ([28d13ba](https://github.com/danielclasen/faderz/commit/28d13badd0960cc789dc8ff2b4810b39c75260af))

## 0.1.0 (2026-11-17)


### Features

* **bridge:** drive MCU surfaces as faderz consoles ([#36](https://github.com/danielclasen/faderz/issues/36)) ([64809ca](https://github.com/danielclasen/faderz/commit/64809cac9c0fbaa3272d7c3d3ab25ce4140e9cd1))
* **bridge:** sign in at the OIDC provider with the device grant ([#37](https://github.com/danielclasen/faderz/issues/37)) ([66f032f](https://github.com/danielclasen/faderz/commit/66f032fe90dd41bc6164fdb7d7bc9969cfb28e12))
