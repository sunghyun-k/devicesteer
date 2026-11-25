# Third-party notices

DeviceSteer adapts code from the projects below. Files that copy or closely follow their code say
so at the top, with the source path.

## iphone-cli

<https://github.com/HyunSeob/iphone-cli>

Adapted: CoreDevice request building, stopping streams by `RemoteSSRC`, the media-session
bring-up, HID input timing (including the wait after attaching a keyboard), `getdisplayinfo` parsing, the accessibility subscription, NSKeyedArchive reader, dump format,
attribute table and tree walk (`crates/core/src/coredevice.rs`, `media.rs`, `input.rs`,
`display.rs`, `ax/`, `video.rs`, `mp4.rs`, `record.rs`).

```text
MIT License

Copyright (c) 2026 HyunSeob

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Geist and Geist Mono

<https://github.com/vercel/geist-font>

The homepage serves the variable Geist and Geist Mono fonts (`site/fonts/`, from the `geist` npm
package 1.3.1) under the SIL Open Font License 1.1; the full license is in `site/fonts/OFL.txt`.
Copyright (c) 2023 Vercel, in collaboration with basement.studio.
