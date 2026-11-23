Quick Enhancer — third-party software and notices
=================================================

Quick Enhancer itself is a small Windows program written in Rust. To process
images and video it uses the free tools, runtimes and AI models listed below.
They are NOT bundled in the installer: the app downloads them on request
(pinned versions, verified with SHA-256) from their official release pages,
from PyPI, or from this project's "models-YYYY.MM" release (converted model
files, hosted only where the model's license allows redistribution), or
installs them from archives you downloaded yourself. Each item keeps its own
license; the AI models page shows the license and credits of every model, and
a copy of each tool license is installed into the "licenses" folder.

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

RIFE ncnn Vulkan 20250112, TNTwise fork (rife-ncnn-vulkan.exe + models
rife-v4.26, rife-v4.25-lite, rife-v4.6, rife-v4)
  Project:  https://github.com/TNTwise/rife-ncnn-vulkan (fork of
            https://github.com/nihui/rife-ncnn-vulkan)
  Authors:  nihui (ncnn port), TNTwise (newer models); RIFE by Zhewei Huang,
            Tianyuan Zhang, Wen Heng, Boxin Shi, Shuchang Zhou (hzwer / Megvii
            Research), https://github.com/hzwer/Practical-RIFE
  License:  MIT. Repacked (the exe and the four models only) on this project's
            models release. Earlier versions of Quick Enhancer used nihui's
            20221029 build.

Real-CUGAN ncnn Vulkan 20220728 (realcugan-ncnn-vulkan.exe + models-se, models-pro)
  Project:  https://github.com/nihui/realcugan-ncnn-vulkan
  Authors:  nihui (ncnn port); Real-CUGAN by bilibili AI Lab,
            https://github.com/bilibili/ailab
  License:  MIT.

ONNX runtimes (downloaded on request, one of them)
--------------------------------------------------

ONNX Runtime 1.24.4 with DirectML (onnxruntime.dll, DirectML.dll)
  Project:  https://onnxruntime.ai (Microsoft); from the onnxruntime-directml
            wheel on PyPI
  License:  ONNX Runtime: MIT. DirectML.dll: Microsoft software license terms
            (redistributable as part of an application). Quick Enhancer downloads
            it from PyPI instead of hosting it.

ONNX Runtime 1.32.0 with CUDA (NVIDIA cards only, optional)
  Project:  https://onnxruntime.ai; onnxruntime-gpu wheel on PyPI, plus NVIDIA's
            nvidia-cuda-runtime 13.5.37, nvidia-cublas 13.10.0.20,
            nvidia-cuda-nvrtc 13.5.37 and nvidia-cudnn-cu13 9.28.1.4 wheels
  License:  ONNX Runtime: MIT. CUDA libraries: NVIDIA Software License
            Agreement (CUDA Toolkit EULA). cuDNN: NVIDIA cuDNN Software License
            Agreement. Downloaded from PyPI, not hosted by this project.

AI models (downloaded on request from the AI models page)
---------------------------------------------------------

Real-ESRGAN models (realesrgan-x4plus, -x4plus-anime, realesr-animevideov3)
  Xintao Wang et al. — BSD 3-Clause (inside the Real-ESRGAN archive above).

Real-CUGAN models (SE, Pro) — bilibili AI Lab — MIT (inside the archive above).

Models by Philip Hofmann (Phhofm), https://github.com/Phhofm/models
  License:  Creative Commons Attribution 4.0 (CC-BY-4.0),
            https://creativecommons.org/licenses/by/4.0/
  Converted by this project (PyTorch -> ONNX or ncnn; scripts in tools/convert/):
    4xNomos2_realplksr_dysample  -> realplksr-nomos2-4x-s256.onnx (static 256)
    2xNomosUni_span_multijpg_ldl -> span-nomosuni-2x.onnx
    4xNomos2_hq_dat2             -> dat2-nomos2-4x-s256-fp16.onnx (fp16, static 256)
    4xBHI_small_hat-l            -> hatl-bhi-small-4x-s128.onnx (static 128)
    1xDeJPG_realplksr_otf        -> dejpg-realplksr-1x-fp16.onnx (fp16)
    1xDeNoise_realplksr_otf      -> denoise-realplksr-1x-fp16.onnx (fp16)
    4xNomos2_otf_esrgan          -> esrgan-nomos2-4x-ncnn.zip (ncnn fp16)

SCUNet (scunet_color_real_psnr) — Kai Zhang et al., https://github.com/cszn/SCUNet
  License:  Apache-2.0. Converted to ONNX (fp16, static 256).

GFPGAN 1.4 — Xintao Wang et al., Tencent ARC, https://github.com/TencentARC/GFPGAN
  License:  Apache-2.0, with the third-party notices in GFPGAN's LICENSE.
            Converted to ONNX (float32 only, input / output scaled to 0..1).

CodeFormer 0.1.0 — Shangchen Zhou et al., S-Lab, Nanyang Technological University,
https://github.com/sczhou/CodeFormer
  License:  S-Lab License 1.0 — NON-COMMERCIAL USE ONLY. The app asks you to
            confirm this before installing it. Converted to ONNX (input / output
            scaled to 0..1, fidelity "weight" input).

YuNet face detector (2023mar) — Shiqi Yu et al., OpenCV Zoo,
https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet
  License:  MIT. Unchanged copy.

The models release also contains MODELS-LICENSES.md and the full license texts.
Images and videos you make with these models are yours; the licenses above
govern the model files (CodeFormer: not for commercial use).

Embedded in the program
-----------------------

Noto Sans Thai (font, used to display Thai text)
  Copyright 2022 The Noto Project Authors (https://github.com/notofonts/thai)
  License:  SIL Open Font License 1.1 — full text in NotoSansThai-OFL.txt.

Rust libraries
  The program is built from open-source Rust crates (egui/eframe, wgpu, ash,
  image, rodio/cpal, ureq/rustls, zip, sha2, serde, ort, half, and
  others) published under MIT and/or Apache-2.0 (and compatible permissive
  licenses such as BSD, Zlib, ISC, Unicode-3.0). A complete list with license
  texts can be generated with `cargo about` or `cargo license` from the source
  tree (see docs/BUILDING.md).

Network use and privacy
-----------------------

Quick Enhancer contains no telemetry. It connects to the network only:
  * when you download or repair tools, runtimes or models (the GitHub release
    pages and PyPI files listed above), including a refresh of the model list
    (models.json) from this project's models release, and
  * when you check for updates (GitHub Releases of Quick Enhancer), either by
    pressing "Check for updates" or, if you opted in, once at startup.
No information about you or your files is sent.
