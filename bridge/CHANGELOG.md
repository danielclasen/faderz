# Changelog

## [0.4.3](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.4.2...faderz-bridge-v0.4.3) (2026-11-27)


### Features

* **bridge:** colour an X-Touch's scribble strips from the channels ([#212](https://github.com/danielclasen/faderz/issues/212)) ([6762e8a](https://github.com/danielclasen/faderz/commit/6762e8a1129549c212c0bb75c48ecd8d94dbd6f7))
* **bridge:** show the time of day on the X-Touch's timecode display ([#218](https://github.com/danielclasen/faderz/issues/218)) ([d02242b](https://github.com/danielclasen/faderz/commit/d02242be0f26ed529854b3533282778ae6a2abe8))


### Bug Fixes

* **bridge:** lay the sign-in out on the scribble strip's 7-character cells ([#217](https://github.com/danielclasen/faderz/issues/217)) ([e4a0518](https://github.com/danielclasen/faderz/commit/e4a05183bb0b13530a5f12a8d4b58c78a570148a))
* **bridge:** send a surface its state again, a little at a time ([#216](https://github.com/danielclasen/faderz/issues/216)) ([0d08c4d](https://github.com/danielclasen/faderz/commit/0d08c4d3bd89cf1dae61ab9c3533f25b42ad9642))
* **bridge:** send RTP-MIDI packets of at most 64 octets ([#208](https://github.com/danielclasen/faderz/issues/208)) ([d5d0f6d](https://github.com/danielclasen/faderz/commit/d5d0f6d47cd0299cbe2252b823e7ff4a25b52abd))

## [0.4.2](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.4.1...faderz-bridge-v0.4.2) (2026-11-25)


### Features

* **bridge:** light the strip meters and CLIP of an MCU surface ([#195](https://github.com/danielclasen/faderz/issues/195)) ([5606d02](https://github.com/danielclasen/faderz/commit/5606d02251faeca2925727b2fdd3c793cc753909))
* share the selection between a surface and its user's screens ([#196](https://github.com/danielclasen/faderz/issues/196)) ([d2dd27f](https://github.com/danielclasen/faderz/commit/d2dd27f4d5271aeb9d10ed6fb14044aef07f13fb))

## [0.4.1](https://github.com/danielclasen/faderz/compare/faderz-bridge-v0.4.0...faderz-bridge-v0.4.1) (2026-11-25)


### Features

* **bridge:** put the V-Pots on the channels' pan ([#193](https://github.com/danielclasen/faderz/issues/193)) ([7ad4525](https://github.com/danielclasen/faderz/commit/7ad45252b4fcecda2a4494d074ab58497e86cd81))

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
