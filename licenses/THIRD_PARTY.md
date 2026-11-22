Quick Enhancer — third-party software and notices
=================================================

Quick Enhancer itself is a small Windows program written in Rust. To process
images and video it uses the free tools listed below. They are NOT bundled in
the installer: the app downloads them on request from their official release
pages (pinned versions, verified with SHA-256) or installs them from archives
you downloaded yourself. Each tool keeps its own license; a copy of each
license is installed into the "licenses" folder next to the tools.

External tools (downloaded on request)
--------------------------------------

FFmpeg 9.1 "essentials" build (ffmpeg.exe, ffprobe.exe)
  Project:  https://ffmpeg.org
  Build:    Gyan Doshi, https://www.gyan.dev/ffmpeg/builds/
            (archive mirrored at https://github.com/GyanD/codexffmpeg)
  License:  GNU GPL version 3. This build is configured with GPL components
            (for example libx264 and libx265), so the build as a whole is GPL-3.0.
            Source code: https://ffmpeg.org/download.html and the build's
            README.txt (installed to licenses/ffmpeg/).
  Quick Enhancer runs ffmpeg/ffprobe as separate programs and does not link
  against them.

Real-ESRGAN ncnn Vulkan 0.2.5.0 (realesrgan-ncnn-vulkan.exe + models)
  Project:  https://github.com/xinntao/Real-ESRGAN
  Authors:  Xintao Wang, Liangbin Xie, Chao Dong, Ying Shan
  License:  BSD 3-Clause. Uses ncnn (Tencent, BSD 3-Clause) and the
            Microsoft Visual C++ OpenMP runtime (vcomp140.dll) shipped in the
            official release archive.

RIFE ncnn Vulkan 20221029 (rife-ncnn-vulkan.exe + models)
  Project:  https://github.com/nihui/rife-ncnn-vulkan
  Authors:  nihui (ncnn port); RIFE by Zhewei Huang, Tianyuan Zhang, Wen Heng,
            Boxin Shi, Shuchang Zhou (hzwer / Megvii Research),
            https://github.com/hzwer/Practical-RIFE
  License:  MIT.

Embedded in the program
-----------------------

Noto Sans Thai (font, used to display Thai text)
  Copyright 2022 The Noto Project Authors (https://github.com/notofonts/thai)
  License:  SIL Open Font License 1.1 — full text in NotoSansThai-OFL.txt.

Rust libraries
  The program is built from open-source Rust crates (egui/eframe, wgpu, ash,
  image, rodio/cpal, ureq/rustls, zip, sha2, serde, and
  others) published under MIT and/or Apache-2.0 (and compatible permissive
  licenses such as BSD, Zlib, ISC, Unicode-3.0). A complete list with license
  texts can be generated with `cargo about` or `cargo license` from the source
  tree (see docs/BUILDING.md).

Network use and privacy
-----------------------

Quick Enhancer contains no telemetry. It connects to the network only:
  * when you download or repair tools (GitHub release pages listed above), and
  * when you check for updates (GitHub Releases of Quick Enhancer), either by
    pressing "Check for updates" or, if you opted in, once at startup.
No information about you or your files is sent.
