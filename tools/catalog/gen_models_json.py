"""Generate models.json (the embedded model catalog) from the tables below.

Run from the repository root:  python tools/catalog/gen_models_json.py models.json

Hashes and sizes are the real values of the files uploaded to the `models-2026.11` release
(or of the pinned upstream files). Update them together with the uploaded files, then run
`cargo test` (the catalog tests validate the result).
"""
import json
import sys

REL = "https://github.com/NobpasinTumdee/quick-enhancer/releases/download/models-2026.11/"
HF = "https://huggingface.co/moonbox-dev/SeedVR2-3B-ONNX/resolve/e2a111d55922e9059a0d663bc189da766fc6b775/int8/"
PYPI = "https://files.pythonhosted.org/packages/"


def rule(src, dst):
    return {"from": src, "to": dst}


def file_pkg(id, name, version, file, sha, size, to, license, homepage, url=None):
    return dict(id=id, name=name, version=version, required=False, platform="any",
                url=url or REL + file, sha256=sha, size=size, archive="file",
                license=license, homepage=homepage, files=[rule(file, to)])


def wheel(id, name, version, path, sha, size, license, homepage, files):
    return dict(id=id, name=name, version=version, required=False, platform="windows-x64",
                url=PYPI + path, sha256=sha, size=size, archive="zip",
                license=license, homepage=homepage, files=files)


NV = "NVIDIA Software License Agreement (CUDA Toolkit EULA)"
packages = [
    # --- ONNX Runtime: one runtime per process (DirectML for any DX12 GPU, or CUDA for NVIDIA) ---
    wheel("ort-dml", "ONNX Runtime 1.24.4 + DirectML", "1.24.4",
          "88/ea/33814eb0ec96775eda4c1d30b0d86e91d7d2cd0d84c66d3915aef0e06fa3/onnxruntime_directml-1.24.4-cp312-cp312-win_amd64.whl",
          "f2ecb68b7b7b259d2ef3112ae760149f9b5a1e7c0fbb73d539da6250a648a614", 25111930,
          "MIT (ONNX Runtime, Microsoft); DirectML: Microsoft software license", "https://onnxruntime.ai",
          [rule("onnxruntime/capi/onnxruntime.dll", "runtime/dml/onnxruntime.dll"),
           rule("onnxruntime/capi/onnxruntime_providers_shared.dll", "runtime/dml/onnxruntime_providers_shared.dll"),
           rule("onnxruntime/capi/DirectML.dll", "runtime/dml/DirectML.dll"),
           rule("onnxruntime/LICENSE", "licenses/onnxruntime/LICENSE"),
           rule("onnxruntime/ThirdPartyNotices.txt", "licenses/onnxruntime/ThirdPartyNotices.txt")]),
    wheel("ort-cuda", "ONNX Runtime 1.32.0 (CUDA)", "1.32.0",
          "ee/3b/6ce3c2548db5f8f3e639373640fa7f7fcdbcbce7b754971c0b293ab105b5/onnxruntime_gpu-1.32.0-cp312-cp312-win_amd64.whl",
          "d8bc5d7668872889d605c1e7253581e021db7dea41a142ad2ab78d6d96f3f289", 157765830,
          "MIT (ONNX Runtime, Microsoft)", "https://onnxruntime.ai",
          [rule("onnxruntime/capi/onnxruntime.dll", "runtime/cuda/onnxruntime.dll"),
           rule("onnxruntime/capi/onnxruntime_providers_shared.dll", "runtime/cuda/onnxruntime_providers_shared.dll"),
           rule("onnxruntime/capi/onnxruntime_providers_cuda.dll", "runtime/cuda/onnxruntime_providers_cuda.dll"),
           rule("onnxruntime/LICENSE", "licenses/onnxruntime-cuda/LICENSE"),
           rule("onnxruntime/ThirdPartyNotices.txt", "licenses/onnxruntime-cuda/ThirdPartyNotices.txt")]),
    wheel("cuda-runtime", "NVIDIA CUDA 13 runtime", "13.5.37",
          "35/99/e07dd9565d2e0317c1a62ff39ae9fc0542c048f74bcfc777c8be97710347/nvidia_cuda_runtime-13.5.37-py3-none-win_amd64.whl",
          "f225d3fa5ea6bfbb58fc4979ef72375cbe661d9107f0d8a4ff22cc151525e4a9", 2780254, NV,
          "https://pypi.org/project/nvidia-cuda-runtime/",
          [rule("nvidia/cu13/bin/x86_64/cudart64_13.dll", "runtime/cuda/cudart64_13.dll"),
           rule("nvidia_cuda_runtime-13.5.37.dist-info/licenses/License.txt", "licenses/nvidia/cuda-runtime.txt")]),
    wheel("cuda-cublas", "NVIDIA cuBLAS 13", "13.10.0.20",
          "12/39/e1b1b4adb8b171894d9a89035df0596d448cf73dc403802d24dbe30cc0be/nvidia_cublas-13.10.0.20-py3-none-win_amd64.whl",
          "c310b749eea855b88fafb906de08d6819b87216c6711640771764fea5dcd8546", 427284863, NV,
          "https://pypi.org/project/nvidia-cublas/",
          [rule("nvidia/cu13/bin/x86_64/cublas64_13.dll", "runtime/cuda/cublas64_13.dll"),
           rule("nvidia/cu13/bin/x86_64/cublasLt64_13.dll", "runtime/cuda/cublasLt64_13.dll"),
           rule("nvidia_cublas-13.10.0.20.dist-info/licenses/License.txt", "licenses/nvidia/cublas.txt")]),
    wheel("cuda-nvrtc", "NVIDIA NVRTC 13", "13.5.37",
          "9f/6c/241c5155199a6de55bad8862213ed64b487dc9526aae817e4af31afea1a4/nvidia_cuda_nvrtc-13.5.37-py3-none-win_amd64.whl",
          "e8e6c10b251385707ae3abd7b4ee118de6736fd7678d0f0f4eab4d759f679c21", 49248991, NV,
          "https://pypi.org/project/nvidia-cuda-nvrtc/",
          [rule("nvidia/cu13/bin/x86_64/nvrtc64_130_0.dll", "runtime/cuda/nvrtc64_130_0.dll"),
           rule("nvidia/cu13/bin/x86_64/nvrtc-builtins64_135.dll", "runtime/cuda/nvrtc-builtins64_135.dll"),
           rule("nvidia_cuda_nvrtc-13.5.37.dist-info/licenses/License.txt", "licenses/nvidia/nvrtc.txt")]),
    wheel("cuda-cudnn", "NVIDIA cuDNN 9", "9.28.1.4",
          "48/9c/26e6bdd31c97c389aa565b49c916be0dec17dd61dad6cd33fee286d1b946/nvidia_cudnn_cu13-9.28.1.4-py3-none-win_amd64.whl",
          "73d874c6bd8a1c75ca040228f77c4e163efe9bab3b65bd85914e2b7334ac68ee", 407065099,
          "NVIDIA cuDNN Software License Agreement", "https://pypi.org/project/nvidia-cudnn-cu13/",
          [rule("nvidia/cudnn/bin/", "runtime/cuda/"),
           rule("nvidia_cudnn_cu13-9.28.1.4.dist-info/licenses/LICENSE", "licenses/nvidia/cudnn.txt")]),
    # --- Real-CUGAN ncnn-vulkan (tool + models, pinned upstream release) ---
    dict(id="realcugan", name="Real-CUGAN ncnn-vulkan + models", version="20220728", required=False,
         platform="windows-x64",
         url="https://github.com/nihui/realcugan-ncnn-vulkan/releases/download/20220728/realcugan-ncnn-vulkan-20220728-windows.zip",
         sha256="c6e08d46c11704b1e3a1ada9ddd591cb5005f52f132136c8633ba25def400e01", size=45977449, archive="zip",
         license="MIT (realcugan-ncnn-vulkan, nihui; Real-CUGAN models, bilibili)",
         homepage="https://github.com/nihui/realcugan-ncnn-vulkan",
         files=[rule("*/realcugan-ncnn-vulkan.exe", "bin/realcugan-ncnn-vulkan.exe"),
                rule("*/vcomp140.dll", "bin/vcomp140.dll"),
                rule("*/models-se/", "models/realcugan/models-se/"),
                rule("*/models-pro/", "models/realcugan/models-pro/"),
                rule("*/LICENSE", "licenses/realcugan/LICENSE")]),
    # --- models we converted (hosted on our models release) ---
    dict(id="esrgan-nomos2-ncnn", name="4xNomos2 ESRGAN (ncnn)", version="2026.11", required=False, platform="any",
         url=REL + "esrgan-nomos2-4x-ncnn.zip",
         sha256="b8605a80f074b54c7d69768f90d82fcf1a5cef93d8497fdbb4a5601d9dd482a1", size=30965044, archive="zip",
         license="CC-BY-4.0 (Phhofm)", homepage="https://github.com/Phhofm/models",
         files=[rule("4xNomos2_otf_esrgan.param", "models/realesrgan/4xNomos2_otf_esrgan.param"),
                rule("4xNomos2_otf_esrgan.bin", "models/realesrgan/4xNomos2_otf_esrgan.bin"),
                rule("LICENSE.txt", "licenses/models/4xNomos2_otf_esrgan.txt")]),
]
PHH = ("CC-BY-4.0 (Phhofm)", "https://github.com/Phhofm/models")
for pid, name, file, sha, size, lic in [
    ("onnx-realplksr-nomos2", "4xNomos2 RealPLKSR (ONNX)", "realplksr-nomos2-4x-s256.onnx",
     "6a59797a52a7057cb6c1db762cf96862bb5a9c71e3105990ea7ba9c4b2609cde", 29914682, PHH),
    ("onnx-span-nomosuni", "2xNomosUni SPAN (ONNX)", "span-nomosuni-2x.onnx",
     "ba2405245d0d5f7e9a2e81dc37270271b175841b8ae41321cc1e78e9a217c1fb", 1656556, PHH),
    ("onnx-dat2-nomos2", "4xNomos2 DAT2 (ONNX fp16)", "dat2-nomos2-4x-s256-fp16.onnx",
     "8bda37463f7d03a9e472a0bc60ad6f0dcade643197a35036696e41258fc326ff", 42233621, PHH),
    ("onnx-hatl-bhi", "4xBHI small HAT-L (ONNX)", "hatl-bhi-small-4x-s128.onnx",
     "70c3ba2dbdf1a024e2e78600219622dd12d65dc764faca4ae906ef39291043d0", 319120436, PHH),
    ("onnx-dejpg", "1xDeJPG RealPLKSR (ONNX fp16)", "dejpg-realplksr-1x-fp16.onnx",
     "18ad3f60181bc8dc2d4af775fa9a37fe78c2ea413dde2ef22b9ec4f1c1002126", 15070822, PHH),
    ("onnx-denoise", "1xDeNoise RealPLKSR (ONNX fp16)", "denoise-realplksr-1x-fp16.onnx",
     "2dfd6c31e39a735239cd5285902f568c0ec89557cc01327305ab5086cc6e797c", 15070822, PHH),
    ("onnx-scunet", "SCUNet real PSNR (ONNX fp16)", "scunet-real-psnr-s256-fp16.onnx",
     "6f8cf9abdf770fcf470e1607fc0750cec516d42c41f8945016932e6a6c25e473", 59506978,
     ("Apache-2.0 (Kai Zhang, KAIR)", "https://github.com/cszn/SCUNet")),
    ("onnx-yunet", "YuNet face detector (2023mar)", "yunet-2023mar.onnx",
     "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4", 232589,
     ("MIT (OpenCV Zoo, Shiqi Yu)", "https://github.com/opencv/opencv_zoo")),
    ("onnx-gfpgan", "GFPGAN 1.4 (ONNX)", "gfpgan-1.4-512.onnx",
     "fa44832c616278cc9daacf6433c4be0c38c0856d30254caae1bd78c38fab32ee", 337595396,
     ("Apache-2.0 (TencentARC)", "https://github.com/TencentARC/GFPGAN")),
    ("onnx-codeformer", "CodeFormer 0.1.0 (ONNX)", "codeformer-0.1.0-512.onnx",
     "e8b5d90544cfd3a80d60570f2d6250605c51f08f366d6a243f33ecf1cf9b6410", 376839216,
     ("S-Lab License 1.0 (non-commercial)", "https://github.com/sczhou/CodeFormer")),
]:
    packages.append(file_pkg(pid, name, "2026.11", file, sha, size, "models/onnx/" + file, lic[0], lic[1]))

SEED = [("dit.onnx", "e73874804e6062a81ea3a047ed74e671f27d54fee158408b04d803b0f6c95d21", 1630270),
        ("dit.onnx.data", "c36caaa979408e6b3ad6c956ba55e92d9120ae0c84ac62bc8537543d6e79790f", 3555440304),
        ("vae_enc_first.onnx", "d6a9041ed8a4eacf403229894704327dab9275218ac933dcd54cd44cb0db6dbe", 200468788),
        ("vae_enc_next.onnx", "bcffa79fd16db2acc4953c043722f26defaa84723865a64a0a9cfe3af64f45d2", 200339836),
        ("vae_dec_first.onnx", "8250314ef04dcae8adf28b5209baa254ba478537257206cce5f6670cd7eb05a7", 301715063),
        ("vae_dec_next.onnx", "e300dd92f6f72a1bd2857fd3bbe04c5e31b71a93b88edd0ed390028baf46ff73", 301538837),
        ("LICENSE.txt", "fa62fea050d25dbb5915b80cbb0a27b43317fe3c3fc614c0142b7d7429b24a3c", 11535)]
for f, sha, size in SEED:
    packages.append(file_pkg("seedvr2-" + f.replace(".", "-").lower(), f"SeedVR2-3B int8: {f}", "e2a111d", f, sha,
                             size, "models/seedvr2/int8/" + f,
                             "Apache-2.0 (ByteDance Seed; ONNX export by moonbox-dev)",
                             "https://huggingface.co/moonbox-dev/SeedVR2-3B-ONNX", url=HF + f))


def T(en, th):
    return dict(en=en, th=th)


def lic(name, url, attribution, commercial=True):
    return dict(name=name, url=url, commercial=commercial, redistribute=True, attribution=attribution)


def phh(tag):
    return lic("CC-BY-4.0", "https://creativecommons.org/licenses/by/4.0/",
               f"{tag} by Philip Hofmann (Phhofm), https://github.com/Phhofm/models/releases/tag/{tag}")


ESR = lic("BSD-3-Clause", "https://github.com/xinntao/Real-ESRGAN/blob/master/LICENSE", "Real-ESRGAN by Xintao Wang et al.")
CUGAN = lic("MIT", "https://github.com/bilibili/ailab/blob/main/LICENSE", "Real-CUGAN by bilibili AI Lab; ncnn port by nihui")
RIFE = lic("MIT", "https://github.com/hzwer/ECCV2022-RIFE/blob/main/LICENSE",
           "RIFE by hzwer / Megvii; ncnn port by nihui (TNTwise fork)")


def ncnn(id, group, backend, name, desc, ncnn_name, scales, speed, quality, vmin, vrec, s_mp, license, src, pkgs,
         files, content, **kw):
    return dict(id=id, group=group, name=name, desc=desc, backend=backend, ncnn_name=ncnn_name, scales=scales,
                speed=speed, quality=quality, vram_min_mb=vmin, vram_rec_mb=vrec,
                cost=dict(s_per_mp=s_mp, vram_mb=vmin), license=license, source=src, packages=pkgs, files=files,
                content=content, **kw)


def onnx(id, group, name, desc, file, scale, tile, static, precision, speed, quality, vmin, vrec, s_mp, license,
         src, pkg, content, range="01", requires=(), **kw):
    spec = dict(file="models/onnx/" + file, tile=tile, static_tile=static, precision=precision, input="input",
                output="output", range=range)
    return dict(id=id, group=group, name=name, desc=desc, backend="onnx", scales=[scale], onnx=spec,
                speed=speed, quality=quality, vram_min_mb=vmin, vram_rec_mb=vrec,
                cost=dict(s_per_mp=s_mp, vram_mb=vmin), license=license, source=src, packages=[pkg],
                files=["models/onnx/" + file], content=content, requires=["runtime:onnx", *requires], **kw)


def esr(m):
    return [f"models/realesrgan/{m}.param", f"models/realesrgan/{m}.bin"]


RE = "https://github.com/xinntao/Real-ESRGAN"
models = [
    ncnn("realesrgan-x4plus", "upscale", "ncnn_realesrgan", T("Real-ESRGAN x4plus", "Real-ESRGAN x4plus"),
         T("Classic general-purpose 4x model for photos. Fast and safe.",
           "โมเดล 4x ทั่วไปสำหรับรูปถ่าย เร็วและเสถียร"),
         "realesrgan-x4plus", [4], 3, 3, 600, 1500, 6.0, ESR, RE, ["realesrgan"], esr("realesrgan-x4plus"), ["photo"]),
    ncnn("realesrgan-x4plus-anime", "upscale", "ncnn_realesrgan",
         T("Real-ESRGAN x4plus anime", "Real-ESRGAN x4plus anime"),
         T("4x model for anime illustrations and still images.", "โมเดล 4x สำหรับภาพวาดอนิเมะและภาพนิ่ง"),
         "realesrgan-x4plus-anime", [4], 3, 3, 500, 1200, 4.0, ESR, RE, ["realesrgan"],
         esr("realesrgan-x4plus-anime"), ["anime"]),
    ncnn("realesr-animevideov3", "upscale", "ncnn_realesrgan",
         T("Real-ESRGAN animevideo v3", "Real-ESRGAN animevideo v3"),
         T("Small, fast 2x/3x/4x model for anime video.", "โมเดลเล็กและเร็ว 2x/3x/4x สำหรับวิดีโออนิเมะ"),
         "realesr-animevideov3", [2, 3, 4], 5, 3, 300, 800, 0.8, ESR, RE, ["realesrgan"],
         esr("realesr-animevideov3-x2"), ["anime", "video"]),
    ncnn("esrgan-nomos2-4x", "upscale", "ncnn_realesrgan", T("Nomos2 ESRGAN 4x", "Nomos2 ESRGAN 4x"),
         T("Community ESRGAN trained on real-world damage: natural textures for photos.",
           "ESRGAN จากชุมชน ฝึกกับความเสียหายของภาพจริง ได้พื้นผิวที่เป็นธรรมชาติสำหรับรูปถ่าย"),
         "4xNomos2_otf_esrgan", [4], 2, 4, 600, 1500, 12.0, phh("4xNomos2_otf_esrgan"),
         "https://github.com/Phhofm/models", ["realesrgan", "esrgan-nomos2-ncnn"], esr("4xNomos2_otf_esrgan"),
         ["photo"]),
    ncnn("realcugan-se", "upscale", "ncnn_realcugan", T("Real-CUGAN", "Real-CUGAN"),
         T("Anime / line-art model with crisp lines, 2x/3x/4x with denoise levels.",
           "โมเดลอนิเมะ/ลายเส้น เส้นคม 2x/3x/4x พร้อมระดับลด noise"),
         "models-se", [2, 3, 4], 3, 4, 1100, 2100, 1.5, CUGAN, "https://github.com/bilibili/ailab/tree/main/Real-CUGAN",
         ["realcugan"], ["bin/realcugan-ncnn-vulkan.exe", "models/realcugan/models-se/up2x-conservative.param"],
         ["anime", "video"]),
    ncnn("realcugan-pro", "upscale", "ncnn_realcugan", T("Real-CUGAN Pro", "Real-CUGAN Pro"),
         T("Stronger Real-CUGAN variant (2x/3x) for heavily compressed anime.",
           "Real-CUGAN รุ่นแรงขึ้น (2x/3x) สำหรับอนิเมะที่ถูกบีบอัดมาก"),
         "models-pro", [2, 3], 3, 4, 1100, 2100, 1.6, CUGAN, "https://github.com/bilibili/ailab/tree/main/Real-CUGAN",
         ["realcugan"], ["bin/realcugan-ncnn-vulkan.exe", "models/realcugan/models-pro/up2x-conservative.param"],
         ["anime"]),
    onnx("realplksr-nomos2-4x", "upscale", T("Nomos2 RealPLKSR 4x", "Nomos2 RealPLKSR 4x"),
         T("Best balance of quality and speed for photos: natural skin and texture.",
           "สมดุลคุณภาพกับความเร็วที่ดีที่สุดสำหรับรูปถ่าย ผิวและพื้นผิวดูเป็นธรรมชาติ"),
         "realplksr-nomos2-4x-s256.onnx", 4, 256, True, "fp32", 3, 4, 700, 1500, 4.0,
         phh("4xNomos2_realplksr_dysample"), "https://github.com/Phhofm/models", "onnx-realplksr-nomos2", ["photo"]),
    onnx("span-nomosuni-2x", "upscale", T("NomosUni SPAN 2x", "NomosUni SPAN 2x"),
         T("Very fast 2x model that also cleans JPEG artifacts. Good for video.",
           "โมเดล 2x ที่เร็วมาก ลบรอย JPEG ไปด้วย เหมาะกับวิดีโอ"),
         "span-nomosuni-2x.onnx", 2, 512, False, "fp32", 5, 3, 250, 600, 0.35,
         phh("2xNomosUni_span_multijpg_ldl"), "https://github.com/Phhofm/models", "onnx-span-nomosuni",
         ["photo", "video"]),
    onnx("dat2-nomos2-4x", "upscale", T("Nomos2 DAT2 4x (slow, best)", "Nomos2 DAT2 4x (ช้า คุณภาพสูงสุด)"),
         T("Transformer model with the most detail. Slow: meant for still images.",
           "โมเดล Transformer ให้รายละเอียดมากที่สุด ช้า เหมาะกับภาพนิ่ง"),
         "dat2-nomos2-4x-s256-fp16.onnx", 4, 256, True, "fp16", 1, 5, 2300, 3000, 20.0,
         phh("4xNomos2_hq_dat2"), "https://github.com/Phhofm/models", "onnx-dat2-nomos2", ["photo"]),
    onnx("hatl-bhi-4x", "upscale", T("BHI HAT-L 4x (experimental)", "BHI HAT-L 4x (ทดลอง)"),
         T("Very slow transformer model (about 2 minutes per 1080p photo).",
           "โมเดล Transformer ที่ช้ามาก (ราว 2 นาทีต่อรูป 1080p)"),
         "hatl-bhi-small-4x-s128.onnx", 4, 128, True, "fp32", 1, 5, 2100, 3000, 62.0,
         phh("4xBHI_small_hat-l"), "https://github.com/Phhofm/models", "onnx-hatl-bhi", ["photo"], experimental=True),
    onnx("dejpg-1x", "restore", T("DeJPG 1x", "DeJPG 1x"),
         T("Removes JPEG blocks and ringing at the original size.", "ลบรอยบล็อก JPEG และขอบเงาที่ขนาดเดิม"),
         "dejpg-realplksr-1x-fp16.onnx", 1, 512, False, "fp16", 4, 4, 250, 600, 1.4,
         phh("1xDeJPG_realplksr_otf"), "https://github.com/Phhofm/models", "onnx-dejpg", ["photo", "video"]),
    onnx("denoise-1x", "restore", T("DeNoise 1x", "DeNoise 1x"),
         T("Removes sensor noise at the original size.", "ลด noise จากเซนเซอร์ที่ขนาดเดิม"),
         "denoise-realplksr-1x-fp16.onnx", 1, 512, False, "fp16", 4, 4, 250, 600, 1.4,
         phh("1xDeNoise_realplksr_otf"), "https://github.com/Phhofm/models", "onnx-denoise", ["photo", "video"]),
    onnx("scunet-1x", "restore", T("SCUNet denoise 1x", "SCUNet ลด noise 1x"),
         T("Strong denoiser for heavy noise.", "ลด noise ได้แรง เหมาะกับภาพที่ noise เยอะ"),
         "scunet-real-psnr-s256-fp16.onnx", 1, 256, True, "fp16", 3, 4, 1200, 2000, 2.7,
         lic("Apache-2.0", "https://github.com/cszn/SCUNet/blob/main/LICENSE", "SCUNet by Kai Zhang et al."),
         "https://github.com/cszn/SCUNet", "onnx-scunet", ["photo"]),
    onnx("yunet", "helper", T("YuNet face detector", "ตัวตรวจจับใบหน้า YuNet"),
         T("Finds faces for face restoration.", "หาตำแหน่งใบหน้าสำหรับการฟื้นฟูใบหน้า"),
         "yunet-2023mar.onnx", 1, 640, True, "fp32", 5, 3, 50, 100, 0.0,
         lic("MIT", "https://github.com/opencv/opencv_zoo/blob/main/models/face_detection_yunet/LICENSE",
             "YuNet by Shiqi Yu et al. (OpenCV Zoo)"),
         "https://github.com/opencv/opencv_zoo", "onnx-yunet", [], hidden=True),
    onnx("gfpgan-1.4", "face", T("GFPGAN 1.4", "GFPGAN 1.4"),
         T("Restores blurry or compressed faces with a natural look.",
           "ฟื้นฟูใบหน้าที่เบลอหรือถูกบีบอัด ให้ผลที่ดูเป็นธรรมชาติ"),
         "gfpgan-1.4-512.onnx", 1, 512, True, "fp32", 4, 4, 1500, 2000, 0.0,
         lic("Apache-2.0", "https://github.com/TencentARC/GFPGAN/blob/master/LICENSE",
             "GFPGAN by Xintao Wang et al. (TencentARC)"),
         "https://github.com/TencentARC/GFPGAN", "onnx-gfpgan", ["photo", "video"],
         requires=["yunet"]),
    onnx("codeformer", "face", T("CodeFormer (non-commercial)", "CodeFormer (ห้ามใช้เชิงพาณิชย์)"),
         T("Strong face restoration with a fidelity slider. Non-commercial use only.",
           "ฟื้นฟูใบหน้าได้แรง ปรับความใกล้เคียงต้นฉบับได้ ใช้ได้เฉพาะงานที่ไม่ใช่เชิงพาณิชย์"),
         "codeformer-0.1.0-512.onnx", 1, 512, True, "fp32", 3, 5, 1750, 2500, 0.0,
         lic("S-Lab License 1.0", "https://github.com/sczhou/CodeFormer/blob/master/LICENSE",
             "CodeFormer by Shangchen Zhou et al. (S-Lab, NTU)", commercial=False),
         "https://github.com/sczhou/CodeFormer", "onnx-codeformer", ["photo", "video"], range="01",
         requires=["yunet"]),
]
for name, desc, speed, quality, vmin, s in [
    ("rife-v4.26", T("Newest RIFE (default): fewer artifacts in fast motion.",
                     "RIFE รุ่นใหม่ที่สุด (ค่าเริ่มต้น) ภาพแตกน้อยลงเมื่อเคลื่อนไหวเร็ว"), 3, 4, 630, 0.08),
    ("rife-v4.6", T("Fast and reliable for 2x-4x frame rates.",
                    "เร็วและเสถียร สำหรับเพิ่มเฟรม 2x-4x"), 4, 3, 370, 0.07),
    ("rife-v4.25-lite", T("Light RIFE 4.25 for slower GPUs.", "RIFE 4.25 รุ่นเบา สำหรับการ์ดจอที่ช้า"), 5, 3, 300, 0.05),
    ("rife-v4", T("Older RIFE 4.0.", "RIFE 4.0 รุ่นเก่า"), 4, 2, 370, 0.07),
]:
    label = name.replace("rife-v", "RIFE v")
    models.append(ncnn(name, "interpolate", "ncnn_rife", T(label, label), desc, name, [2, 3, 4], speed, quality, vmin,
                       vmin * 2, s, RIFE, "https://github.com/TNTwise/rife-ncnn-vulkan", ["rife"],
                       [f"models/rife/{name}/flownet.param", f"models/rife/{name}/flownet.bin"], ["video"]))

models.append(dict(
    id="seedvr2-3b", group="video", name=T("SeedVR2 3B (experimental)", "SeedVR2 3B (ทดลอง)"),
    desc=T("Diffusion video restoration. NVIDIA (CUDA) only and very heavy: 4.5 GB download.",
           "ฟื้นฟูวิดีโอด้วย diffusion ใช้ได้เฉพาะ NVIDIA (CUDA) และหนักมาก: ดาวน์โหลด 4.5 GB"),
    backend="onnx", scales=[1, 2], speed=1, quality=5, vram_min_mb=10000, vram_rec_mb=12000,
    cost=dict(s_per_mp=400.0, vram_mb=10000),
    license=lic("Apache-2.0", "https://huggingface.co/moonbox-dev/SeedVR2-3B-ONNX",
                "SeedVR2 by ByteDance Seed; ONNX export by moonbox-dev"),
    source="https://huggingface.co/moonbox-dev/SeedVR2-3B-ONNX",
    packages=[p["id"] for p in packages if p["id"].startswith("seedvr2-")],
    files=["models/seedvr2/int8/" + f for f, _, _ in SEED], content=["video"], experimental=True,
    requires=["runtime:cuda"]))

with open(sys.argv[1], "w", encoding="utf-8", newline="\n") as fh:
    json.dump(dict(schema=1, catalog_version="2026.11.1", update_url=REL + "models.json",
                   packages=packages, models=models), fh, ensure_ascii=False, indent=1)
    fh.write("\n")
