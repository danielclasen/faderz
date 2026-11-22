# Changelog

## [0.5.0](https://github.com/danielclasen/faderz/compare/v0.4.10...v0.5.0) (2026-11-22)


### ⚠ BREAKING CHANGES

* **mix:** a direct out at the input tap carries the input before trim, filters and dynamics, where it carried the pre-fader signal.
* **audio:** delay buses and auxes on their outputs
* **audio:** a bus or aux delay takes a line from mxl.outputs.OUTPUT.maxDelayMs; without one only 0 is taken, and a stored one past its line loads and recalls as 0.

### Features

* **afv:** fade the AFV stage over its rise, hold and fall ([#155](https://github.com/danielclasen/faderz/issues/155)) ([65ed78a](https://github.com/danielclasen/faderz/commit/65ed78ada3b49271bf058ae311adf8f8286ffbc6))
* **audio:** apply bus, aux and mix-minus EQ and dynamics ([022b3bf](https://github.com/danielclasen/faderz/commit/022b3bfe68e6776f5a8eb7cf4ca6e08c2fdfe6ce))
* **audio:** count input switches handed over past their margin ([7f96edb](https://github.com/danielclasen/faderz/commit/7f96edb0e6b77a1eb94d3dc351f2acc5f3d58a60))
* **audio:** delay buses and auxes on their outputs ([022b3bf](https://github.com/danielclasen/faderz/commit/022b3bfe68e6776f5a8eb7cf4ca6e08c2fdfe6ce))
* **meters:** measure bus correlation and channel true peak ([#159](https://github.com/danielclasen/faderz/issues/159)) ([cebbc84](https://github.com/danielclasen/faderz/commit/cebbc84ff7ec93d3839f265814195dbf0ffaee27))
* **meters:** meter a channel's input tap before its strip ([#160](https://github.com/danielclasen/faderz/issues/160)) ([5d83392](https://github.com/danielclasen/faderz/commit/5d8339264e0fab841166b34249bbff2e6be1bd16))
* **meters:** run a pre-strip meter only at the input meter tap ([65e72e8](https://github.com/danielclasen/faderz/commit/65e72e88a4504dc656a30d72231c7e0f198014ae))
* **mix:** take a direct out at the input tap before the strip ([65e72e8](https://github.com/danielclasen/faderz/commit/65e72e88a4504dc656a30d72231c7e0f198014ae))
* **nmos:** end a failed activation disabled on every replica ([7f96edb](https://github.com/danielclasen/faderz/commit/7f96edb0e6b77a1eb94d3dc351f2acc5f3d58a60))
* **nmos:** serve the replicated resource versions on every replica ([7f96edb](https://github.com/danielclasen/faderz/commit/7f96edb0e6b77a1eb94d3dc351f2acc5f3d58a60))


### Bug Fixes

* **deps:** take samplez 0.20.0 for joined late fades, rolled by hand-over ([7f96edb](https://github.com/danielclasen/faderz/commit/7f96edb0e6b77a1eb94d3dc351f2acc5f3d58a60))
* **deps:** take samplez 0.22.0 to rebuild a replica diverged while every output is disabled, rolled by hand-over ([a81f0fd](https://github.com/danielclasen/faderz/commit/a81f0fd30cbf6f4f6318b612301e29f2d2428fbc))
* **deps:** take samplez 0.23.0, so every tap follows the channel delay, rolled by hand-over ([65e72e8](https://github.com/danielclasen/faderz/commit/65e72e88a4504dc656a30d72231c7e0f198014ae))
* **deps:** take url 2.5.9 for RUSTSEC-2026-0592 ([65e72e8](https://github.com/danielclasen/faderz/commit/65e72e88a4504dc656a30d72231c7e0f198014ae))
* **nmos:** show the replicated activation on every replica's /active ([#153](https://github.com/danielclasen/faderz/issues/153)) ([1796e53](https://github.com/danielclasen/faderz/commit/1796e537c4e0e661143f8601e41f7b1074eddce8))
* **nmos:** take a scheduled activation of the connection a port holds ([7f96edb](https://github.com/danielclasen/faderz/commit/7f96edb0e6b77a1eb94d3dc351f2acc5f3d58a60))

## [0.4.10](https://github.com/danielclasen/faderz/compare/v0.4.9...v0.4.10) (2026-11-21)


### Features

* **audio:** apply the channel delay for lip sync ([#146](https://github.com/danielclasen/faderz/issues/146)) ([c842d8c](https://github.com/danielclasen/faderz/commit/c842d8c814a6d5ce528e56438fda1bbe45bc9147))
* **audio:** decide input liveness by a majority of the set ([04a760e](https://github.com/danielclasen/faderz/commit/04a760e793cb8eaacc272bf411ebba9cb145d160))
* **audio:** play a line-up tone generator as a channel's input ([04a760e](https://github.com/danielclasen/faderz/commit/04a760e793cb8eaacc272bf411ebba9cb145d160))
* **audio:** play the line-up tone on its target outputs ([04a760e](https://github.com/danielclasen/faderz/commit/04a760e793cb8eaacc272bf411ebba9cb145d160))
* **scenes:** crossfade a scene recall over its crossfade time ([#149](https://github.com/danielclasen/faderz/issues/149)) ([64bcc84](https://github.com/danielclasen/faderz/commit/64bcc84568741d249e1309f83dcebaecdfcaf496))


### Bug Fixes

* **deps:** take samplez 0.19.0 for the keyless hand-over and readiness ([04a760e](https://github.com/danielclasen/faderz/commit/04a760e793cb8eaacc272bf411ebba9cb145d160))

## [0.4.9](https://github.com/danielclasen/faderz/compare/v0.4.8...v0.4.9) (2026-11-20)


### Features

* **audio:** apply the bus and aux true-peak limiter ([#141](https://github.com/danielclasen/faderz/issues/141)) ([1d62235](https://github.com/danielclasen/faderz/commit/1d62235f26764e98aadd06e977061c78fb0fcfb5))

## [0.4.8](https://github.com/danielclasen/faderz/compare/v0.4.7...v0.4.8) (2026-11-20)


### Features

* **web:** ask to reload when a replica runs a new faderz version ([#138](https://github.com/danielclasen/faderz/issues/138)) ([aa23d7a](https://github.com/danielclasen/faderz/commit/aa23d7a9d8d0739cc60a4bcf1e6a961ca962e8d3))

## [0.4.7](https://github.com/danielclasen/faderz/compare/v0.4.6...v0.4.7) (2026-11-20)


### Features

* show loudness gaps and a restarted measurement beside I and LRA ([#133](https://github.com/danielclasen/faderz/issues/133)) ([b7c6be0](https://github.com/danielclasen/faderz/commit/b7c6be02175aece6282fdfab86377d1dcc9a0dc9))


### Bug Fixes

* **web:** send nothing on a socket that has begun to close ([#134](https://github.com/danielclasen/faderz/issues/134)) ([29a51b2](https://github.com/danielclasen/faderz/commit/29a51b2e6db378ef7c952011bdf44c35b00a6000))

## [0.4.6](https://github.com/danielclasen/faderz/compare/v0.4.5...v0.4.6) (2026-11-20)


### Bug Fixes

* **app:** list a replica down only once it is unheard for 1 s ([#129](https://github.com/danielclasen/faderz/issues/129)) ([05717ed](https://github.com/danielclasen/faderz/commit/05717edb422d7d21f219cbeb0fc0a980dcc5d333))

## [0.4.5](https://github.com/danielclasen/faderz/compare/v0.4.4...v0.4.5) (2026-11-19)


### Features

* explain the engine counters and name the input at fault ([#123](https://github.com/danielclasen/faderz/issues/123)) ([58e4132](https://github.com/danielclasen/faderz/commit/58e41321587740ce65d3ea46560712170898adf6))
* show the running build's changelog in About / What's new ([#124](https://github.com/danielclasen/faderz/issues/124)) ([8291c36](https://github.com/danielclasen/faderz/commit/8291c36878f430f4e77e69900c7a63f1e17e8556))
* **web:** group the header into status and controls ([7d7b570](https://github.com/danielclasen/faderz/commit/7d7b5703ad2c7081d2c09cc7d152758901b39760))
* **web:** say what keys each duck group and what it ducks ([7d7b570](https://github.com/danielclasen/faderz/commit/7d7b5703ad2c7081d2c09cc7d152758901b39760))
* **web:** show each bus's width where it is named ([7d7b570](https://github.com/danielclasen/faderz/commit/7d7b5703ad2c7081d2c09cc7d152758901b39760))

## [0.4.4](https://github.com/danielclasen/faderz/compare/v0.4.3...v0.4.4) (2026-11-19)


### Features

* show the faderz and samplez versions the serving replica runs ([#116](https://github.com/danielclasen/faderz/issues/116)) ([e5b28c3](https://github.com/danielclasen/faderz/commit/e5b28c3bc3f93cdaced9fb2f13ff888c6af6bebd))


### Bug Fixes

* **api:** never take the replica's contact lock under the store lock ([#112](https://github.com/danielclasen/faderz/issues/112)) ([ee959b7](https://github.com/danielclasen/faderz/commit/ee959b72537ccd6730c09cb572f66f2d027345b4))
* **nmos:** retry a refused Fire under fresh ids ([6678d82](https://github.com/danielclasen/faderz/commit/6678d829fc7f3affca97db136f41113048b82502))
* **nmos:** show the state's activation in /active after a restart ([6678d82](https://github.com/danielclasen/faderz/commit/6678d829fc7f3affca97db136f41113048b82502))
* **nmos:** take a /bulk cancel only for its own item answered 2xx ([6678d82](https://github.com/danielclasen/faderz/commit/6678d829fc7f3affca97db136f41113048b82502))


### Performance Improvements

* **state:** hold the store lock for a pointer copy, not the state ([#115](https://github.com/danielclasen/faderz/issues/115)) ([1bd1063](https://github.com/danielclasen/faderz/commit/1bd1063fd2f309549a1736f36707c6271db7e61d))

## [0.4.3](https://github.com/danielclasen/faderz/compare/v0.4.2...v0.4.3) (2026-11-19)


### Features

* **mix:** balance a stereo channel and apply its image and the monitor's ([#103](https://github.com/danielclasen/faderz/issues/103)) ([f682151](https://github.com/danielclasen/faderz/commit/f682151652e79e0c0a0f5563c964c669c1d1eec5))
* **mix:** pan a mono channel in the field of a 5.1 or 7.1 bus ([#104](https://github.com/danielclasen/faderz/issues/104)) ([e44204e](https://github.com/danielclasen/faderz/commit/e44204eb3928aa3cc806e1b69d1db5b785d30d96))


### Bug Fixes

* **meters:** read silence for a channel loudness at a closed tap ([#108](https://github.com/danielclasen/faderz/issues/108)) ([dcd3cb7](https://github.com/danielclasen/faderz/commit/dcd3cb7e2ae8286bd9f5cefc00614620b5fb8dd0))
* **ws:** deliver a control request to the holder on whichever replica it is ([#106](https://github.com/danielclasen/faderz/issues/106)) ([23216fc](https://github.com/danielclasen/faderz/commit/23216fcb3891af8cb36eece84a94919714145d0f))

## [0.4.2](https://github.com/danielclasen/faderz/compare/v0.4.1...v0.4.2) (2026-11-19)


### Features

* **audio:** restart a loudness measurement on POST /loudness/reset ([#99](https://github.com/danielclasen/faderz/issues/99)) ([26bf845](https://github.com/danielclasen/faderz/commit/26bf845c48e65f420e0e48b7f3341c5927bfdb4f))

## [0.4.1](https://github.com/danielclasen/faderz/compare/v0.4.0...v0.4.1) (2026-11-19)


### Features

* **mix:** fold an output down to a width narrower than its source ([#87](https://github.com/danielclasen/faderz/issues/87)) ([e990e72](https://github.com/danielclasen/faderz/commit/e990e722e1d8d59c7c27f3577ced73cb562c93c8))
* **mix:** interrupt a mix-minus with talkback and dim the monitor ([#83](https://github.com/danielclasen/faderz/issues/83)) ([7d4d07b](https://github.com/danielclasen/faderz/commit/7d4d07b7014b2f87dcbae317231010a7059ca3e9))
* **mix:** play channel and VCA PFL/AFL on the monitor output ([#80](https://github.com/danielclasen/faderz/issues/80)) ([46919ed](https://github.com/danielclasen/faderz/commit/46919ed7395449e0857aeea95b43e0a5cfe67337))
* **mxl:** label a member flow with its replica ordinal ([#98](https://github.com/danielclasen/faderz/issues/98)) ([397ae08](https://github.com/danielclasen/faderz/commit/397ae0836f8be72ea95855b30db94ab19612711c))


### Bug Fixes

* **deps:** build against samplez 0.12.0 ([#97](https://github.com/danielclasen/faderz/issues/97)) ([e6d8a19](https://github.com/danielclasen/faderz/commit/e6d8a19ffa93203802162906786bbdf8284132f5))
* **nmos:** show a fired scheduled activation as it was asked for ([#96](https://github.com/danielclasen/faderz/issues/96)) ([fb54ee0](https://github.com/danielclasen/faderz/commit/fb54ee00ec6ef10d9a3a29565472191624e36d85))
* **web:** console header, status bar and box fits ([#84](https://github.com/danielclasen/faderz/issues/84)) ([c69819d](https://github.com/danielclasen/faderz/commit/c69819d6763f53457c36b5e7e7afa9d7fc18a2e4))
* **web:** fit a three-digit user count in the key, not the header ([#95](https://github.com/danielclasen/faderz/issues/95)) ([ce05d8d](https://github.com/danielclasen/faderz/commit/ce05d8d7441995d3ef19429b9e2d6adc70f28433))
* **web:** lay out a touch from another client in boxes of its own ([#89](https://github.com/danielclasen/faderz/issues/89)) ([992bed8](https://github.com/danielclasen/faderz/commit/992bed8e9b829b469e6b384593aeef7b5667e247))

## [0.4.0](https://github.com/danielclasen/faderz/compare/v0.3.2...v0.4.0) (2026-11-19)


### ⚠ BREAKING CHANGES

* **nmos:** snapshots carry format 3 with the connections, which faderz 0.2.0 and earlier refuse; DERIVATION_VERSION 2 keeps replicas of both versions out of one set.

### Features

* **nmos:** register channels and outputs and take IS-05 over MXL ([#63](https://github.com/danielclasen/faderz/issues/63)) ([b4bc68d](https://github.com/danielclasen/faderz/commit/b4bc68d827c03c1c79a75add0e3c5b7701b7ad21))


### Bug Fixes

* **app:** list every replica of the set in the replicas message ([#82](https://github.com/danielclasen/faderz/issues/82)) ([571ab29](https://github.com/danielclasen/faderz/commit/571ab29e0f314707eba3682e3ba5b11157ce1928))
* **audio:** drop a follower's input report its engine took back ([#78](https://github.com/danielclasen/faderz/issues/78)) ([39dde77](https://github.com/danielclasen/faderz/commit/39dde77ce92592b42da9bff3952365defbe6effc))
* **web:** retry a standby that landed on the primary's replica at once ([#86](https://github.com/danielclasen/faderz/issues/86)) ([f110759](https://github.com/danielclasen/faderz/commit/f110759ede7a4dc9b2df51828d952e34afbb9f04))

## [0.3.2](https://github.com/danielclasen/faderz/compare/v0.3.1...v0.3.2) (2026-11-19)


### Bug Fixes

* **audio:** follow the faders in a set replica's meters ([#76](https://github.com/danielclasen/faderz/issues/76)) ([f6a28fb](https://github.com/danielclasen/faderz/commit/f6a28fb97b463a43af96ab8c6b58b198ac15ef2f))
* **web:** wait in settled() until the worker took every write ([#72](https://github.com/danielclasen/faderz/issues/72)) ([3ab4b25](https://github.com/danielclasen/faderz/commit/3ab4b25bec3611922c67d9f3226e82aec81bded5))

## [0.3.1](https://github.com/danielclasen/faderz/compare/v0.3.0...v0.3.1) (2026-11-19)


### Bug Fixes

* **deps:** build against samplez 0.8.3 ([#74](https://github.com/danielclasen/faderz/issues/74)) ([1df0168](https://github.com/danielclasen/faderz/commit/1df01685104716b0b832fdc787771e3e419ca626))
* **web:** move a duck group's threshold mark by transform ([#70](https://github.com/danielclasen/faderz/issues/70)) ([768d3d1](https://github.com/danielclasen/faderz/commit/768d3d12af702a20fb8b1f979f941d222cebf175))

## [0.3.0](https://github.com/danielclasen/faderz/compare/v0.2.0...v0.3.0) (2026-11-18)


### ⚠ BREAKING CHANGES

* **audio:** meter frames are format 4; a decoder of format 3 drops them.

### Features

* **api:** report a replica's role, readiness and engine at /status ([c5b1e0c](https://github.com/danielclasen/faderz/commit/c5b1e0cbade9f4c7a459ff0c5a4c5fdb2ceae9d4))
* **audio:** duck targets under their groups' triggers ([2343d4d](https://github.com/danielclasen/faderz/commit/2343d4d789713eb29f3c48e353152d0722469847))
* **bridge:** light duck keys while their group ducks ([2343d4d](https://github.com/danielclasen/faderz/commit/2343d4d789713eb29f3c48e353152d0722469847))
* **web:** show duck key level and gain reduction from meter frames ([2343d4d](https://github.com/danielclasen/faderz/commit/2343d4d789713eb29f3c48e353152d0722469847))
* **web:** summarise each duck group beside the tiles in the masters ([#66](https://github.com/danielclasen/faderz/issues/66)) ([007160f](https://github.com/danielclasen/faderz/commit/007160f22eaef74de6927d4a11ad9792d7774f0e))


### Bug Fixes

* **deps:** build against samplez 0.8.1 ([#69](https://github.com/danielclasen/faderz/issues/69)) ([47e4730](https://github.com/danielclasen/faderz/commit/47e473041a1ef7484b042088060c4d2a885a6c2b))
* **replica:** start contact as the replica starts ([c5b1e0c](https://github.com/danielclasen/faderz/commit/c5b1e0cbade9f4c7a459ff0c5a4c5fdb2ceae9d4))

## [0.2.0](https://github.com/danielclasen/faderz/compare/v0.1.9...v0.2.0) (2026-11-18)


### ⚠ BREAKING CHANGES

* **replica:** snapshots carry a format (2) beside the state. Earlier binaries refuse them; this one still loads snapshots they wrote.

### Features

* **replica:** run the engine in a replicated set ([#55](https://github.com/danielclasen/faderz/issues/55)) ([8503909](https://github.com/danielclasen/faderz/commit/85039090048424143d67def302bc4f22d0856048))

## [0.1.9](https://github.com/danielclasen/faderz/compare/v0.1.8...v0.1.9) (2026-11-18)


### Features

* **web:** set up and operate duck groups ([#58](https://github.com/danielclasen/faderz/issues/58)) ([1cff324](https://github.com/danielclasen/faderz/commit/1cff3240fc9c2e8e81de7115f46ffd00a369fc75))

## [0.1.8](https://github.com/danielclasen/faderz/compare/v0.1.7...v0.1.8) (2026-11-18)


### Features

* **state:** add duck groups to the shape and state ([#53](https://github.com/danielclasen/faderz/issues/53)) ([2520012](https://github.com/danielclasen/faderz/commit/252001226b9ffe05b78273d18e8222a8fc2588c2))

## [0.1.7](https://github.com/danielclasen/faderz/compare/v0.1.6...v0.1.7) (2026-11-17)


### Features

* **replica:** run as a set of several voters ([#47](https://github.com/danielclasen/faderz/issues/47)) ([cc4d3d8](https://github.com/danielclasen/faderz/commit/cc4d3d814f17e13487614b0f298d571543d36013))
* **ws:** send the engine's counters and anchor their panel to the lamp ([#44](https://github.com/danielclasen/faderz/issues/44)) ([f39d264](https://github.com/danielclasen/faderz/commit/f39d2645c730b2397d7a73fa1b36e4b63c32b9bc))


### Bug Fixes

* **web:** show whole values in processing panels and tally chips ([#50](https://github.com/danielclasen/faderz/issues/50)) ([5c19821](https://github.com/danielclasen/faderz/commit/5c198217f070239af8a64e7161f5190451de86fe))

## [0.1.6](https://github.com/danielclasen/faderz/compare/v0.1.5...v0.1.6) (2026-11-17)


### Bug Fixes

* **web:** post-fader channel meters, monitor output label, masters scroll ([#42](https://github.com/danielclasen/faderz/issues/42)) ([7eba59a](https://github.com/danielclasen/faderz/commit/7eba59af3206895942cdca1d84194c55a585758f))

## [0.1.5](https://github.com/danielclasen/faderz/compare/v0.1.4...v0.1.5) (2026-11-17)


### Features

* **bridge:** drive MCU surfaces as faderz consoles ([#36](https://github.com/danielclasen/faderz/issues/36)) ([64809ca](https://github.com/danielclasen/faderz/commit/64809cac9c0fbaa3272d7c3d3ab25ce4140e9cd1))
* **ws:** register control surfaces as consoles ([#35](https://github.com/danielclasen/faderz/issues/35)) ([8424324](https://github.com/danielclasen/faderz/commit/842432485e3a6ecc08836c83d022ada97d7e0e1f))

## [0.1.4](https://github.com/danielclasen/faderz/compare/v0.1.3...v0.1.4) (2026-11-17)


### Features

* **audio:** export each channel's input health on /metrics ([#30](https://github.com/danielclasen/faderz/issues/30)) ([0e536fb](https://github.com/danielclasen/faderz/commit/0e536fb8f7c2eb7a0930547d7842eaabbe62eb57))
* **config:** default readerMarginSamples to 1536 ([21f77f2](https://github.com/danielclasen/faderz/commit/21f77f29873bd6561fa9a791f1bad2da6bf7baef))
* **config:** select a channel's legs from the channels of a wider flow ([#32](https://github.com/danielclasen/faderz/issues/32)) ([2f51317](https://github.com/danielclasen/faderz/commit/2f513177d93aed19027ddbe68bf940e5ed230583))
* **meters:** meter every channel and bus whether an output carries it ([21f77f2](https://github.com/danielclasen/faderz/commit/21f77f29873bd6561fa9a791f1bad2da6bf7baef))
* **ws:** list the sessions connected to a replica in USERS ([#33](https://github.com/danielclasen/faderz/issues/33)) ([abc31c0](https://github.com/danielclasen/faderz/commit/abc31c0da1f4302eea01d3bab256e615e095fb00))

## [0.1.3](https://github.com/danielclasen/faderz/compare/v0.1.2...v0.1.3) (2026-11-17)


### Bug Fixes

* **sync:** size connection deadlines from the measured round trip ([#27](https://github.com/danielclasen/faderz/issues/27)) ([23c354d](https://github.com/danielclasen/faderz/commit/23c354d45eefc5562ce0b14cbe6020005bcb7ea3))

## [0.1.2](https://github.com/danielclasen/faderz/compare/v0.1.1...v0.1.2) (2026-11-16)


### Features

* **auth:** sign in with OpenID Connect without a proxy in front ([#24](https://github.com/danielclasen/faderz/issues/24)) ([a4c58d0](https://github.com/danielclasen/faderz/commit/a4c58d09f340ea478d99154c916190d9504e01a6))

## [0.1.1](https://github.com/danielclasen/faderz/compare/v0.1.0...v0.1.1) (2026-11-16)


### Bug Fixes

* **image:** grant the binary file capabilities for its RT thread ([#22](https://github.com/danielclasen/faderz/issues/22)) ([045c523](https://github.com/danielclasen/faderz/commit/045c523a65fc762bbc368a4dccbae78c6077bd27))

## 0.1.0 (2026-11-16)


### ⚠ BREAKING CHANGES

* hello carries session, kind and mode instead of clientId; touch ownership is reported as an owner record.

### Features

* add the API contract and the web UI with a replicated-backend mock ([#1](https://github.com/danielclasen/faderz/issues/1)) ([33b55e1](https://github.com/danielclasen/faderz/commit/33b55e161dd457abf913332e0ed68669f91bf41e))
* add the faderz backend on samplez ([18fd356](https://github.com/danielclasen/faderz/commit/18fd356f0b5abd4f8ef63ab84dfa6eece4b5636d))
* add the faderz container image ([#10](https://github.com/danielclasen/faderz/issues/10)) ([4765822](https://github.com/danielclasen/faderz/commit/476582234f58e737c7deeb9fc36c4ec260314350))
* **api:** add a channel's source label and scribble strip colour ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **api:** answer auth and browser refusals with the Error body ([d78b2b8](https://github.com/danielclasen/faderz/commit/d78b2b8c6917735d9798ecb154f8a9b427ac8361))
* ask the holder of a control for it, and mark other selections ([#4](https://github.com/danielclasen/faderz/issues/4)) ([c34a5d0](https://github.com/danielclasen/faderz/commit/c34a5d08307c27e70bb0af214e7127c5ff231af6))
* mix the live state through the samplez engine ([#18](https://github.com/danielclasen/faderz/issues/18)) ([1ac7771](https://github.com/danielclasen/faderz/commit/1ac77719f2093f0d830d780840ae740b6dae7bb5))
* run each channel's processing in its input's strip ([#20](https://github.com/danielclasen/faderz/issues/20)) ([2440ff7](https://github.com/danielclasen/faderz/commit/2440ff7b3865881f4f034bfa5b07415a5dda5cdb))
* serve the web UI from the faderz binary ([#9](https://github.com/danielclasen/faderz/issues/9)) ([eb552c4](https://github.com/danielclasen/faderz/commit/eb552c43ad0a9e5eab6f05afbf683061249b76d2))
* start and pause the loudness integration of each output ([#3](https://github.com/danielclasen/faderz/issues/3)) ([09d25af](https://github.com/danielclasen/faderz/commit/09d25af429bb1464de6719971e70e4f5d46a47ca))
* **web:** add bank keys, a channel jump, FOLLOW and VCA spill ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** fit the mixer to touch screens, phones and the keyboard ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** give portrait tablets the view tabs on a second header row ([63fbec5](https://github.com/danielclasen/faderz/commit/63fbec52f179500d92412e3a292c0785dafaf375))
* **web:** keep the controls usable while nothing commits ([#5](https://github.com/danielclasen/faderz/issues/5)) ([2a1572f](https://github.com/danielclasen/faderz/commit/2a1572f46d3d50001246a167ce5b050193fc34f1))
* **web:** latch fine travel with a FINE key for touch screens ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** list who works on the mixer under a USERS key ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** lock the desk with a hold and mark recall-safe strips ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** make the strip's processing and route chips buttons ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** pass the dev server's API to a faderz backend ([18fd356](https://github.com/danielclasen/faderz/commit/18fd356f0b5abd4f8ef63ab84dfa6eece4b5636d))
* **web:** put an aux's send levels on the channel faders ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** say on return that a talk ended while the page was away ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** show a title on a long press, as touch has no hover ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** switch the theme from the Setup view ([63fbec5](https://github.com/danielclasen/faderz/commit/63fbec52f179500d92412e3a292c0785dafaf375))
* **ws:** deliver selections, control requests and replicas to clients ([d78b2b8](https://github.com/danielclasen/faderz/commit/d78b2b8c6917735d9798ecb154f8a9b427ac8361))


### Bug Fixes

* **audio:** hold the resend lock while marking the engine started ([#21](https://github.com/danielclasen/faderz/issues/21)) ([d4a012d](https://github.com/danielclasen/faderz/commit/d4a012d6fe0683aaf5d6e7eb6663105800446434))
* older Safari WebSockets under the CSP, font licences per family ([#13](https://github.com/danielclasen/faderz/issues/13)) ([3ec2359](https://github.com/danielclasen/faderz/commit/3ec2359de705a3d085721f4063d6b373d8a4c309))
* **ui:** name the WebSocket host in the CSP only as CSP can take it ([#14](https://github.com/danielclasen/faderz/issues/14)) ([e0b8808](https://github.com/danielclasen/faderz/commit/e0b880820040106902af9a7222d8399df8c823f2))
* **web:** cut a strip to five tab stops and add skip links ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** drag processing parameters and send levels relatively ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** fit the header and bank bar to every width from 320 px ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** keep a strip at five tab stops while its fader is held ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** keep amber for PFL and light other keys in a neutral colour ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** keep the dev mock key off controls and readouts ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** keep the faders on screen on a phone in landscape ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** keep the selected channel on a second press, step prev/next ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** keep the tally text in place when the tally changes ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** label channel meters in dBFS, apart from the fader scale ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** mark every state-changing request as JSON ([18fd356](https://github.com/danielclasen/faderz/commit/18fd356f0b5abd4f8ef63ab84dfa6eece4b5636d))
* **web:** move a pan on touch only after the finger rests on it ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** move focus into the processing panel and back to its strip ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** name recall-safe groups in a strip head's tooltip ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** name the compact header keys and keep the notices' log mounted ([#17](https://github.com/danielclasen/faderz/issues/17)) ([a6306da](https://github.com/danielclasen/faderz/commit/a6306da3c8f1f48b15debe1c935eb6d000c0f7ea))
* **web:** name the tally and the fader's holder on the strip in words ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** never step a parameter against its arrow key ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** open the processing panel over the strips, not the masters ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** put replica detail behind the connection lamp ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** raise faint text, key edges and lit-key ink to WCAG AA ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** read a notice on a short screen, announce the commit banner once ([#16](https://github.com/danielclasen/faderz/issues/16)) ([7da4201](https://github.com/danielclasen/faderz/commit/7da4201b7c519bad6af752aad17e651503226b2c))
* **web:** refuse a gesture on a control another session holds ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** replace the counter row with one engine lamp ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** reset a loudness measurement only after a one second hold ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** say in the header when changes are not committing ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** scroll to the right strip while a VCA is spilled ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** separate alternate strips in the light theme ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** set no operator-readable text below 10 px ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** show a fader's holder by tag, with the full name in a tooltip ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** show a locked desk's key on a phone and label it LOCK DESK ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** show a whole bank on a tablet, masters over the strips ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** size every touch control to at least 24 px ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))
* **web:** size the most used keys for a finger on touch screens ([bfd4cc3](https://github.com/danielclasen/faderz/commit/bfd4cc3404828d3560256b4510e35071e4208c93))


### Performance Improvements

* **web:** size each meter canvas to the meters it draws ([#7](https://github.com/danielclasen/faderz/issues/7)) ([198130a](https://github.com/danielclasen/faderz/commit/198130aadbd55ab109cc863ac9a3779e66addea4))
