# Changelog

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
