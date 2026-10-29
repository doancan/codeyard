# Build crates/app/assets/windows/codeyard.ico from codeyard.svg next to it, one frame per size Windows asks for.
# Needs Python with Pillow and resvg-py: pip install pillow resvg-py
import io
import pathlib

import resvg_py
from PIL import Image

DIR = pathlib.Path(__file__).resolve().parent.parent / "crates" / "app" / "assets" / "windows"
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

svg = (DIR / "codeyard.svg").read_text(encoding="utf-8")
# Each size is rendered from the vector, not scaled down from a large frame, so small frames stay sharp.
frames = [Image.open(io.BytesIO(bytes(resvg_py.svg_to_bytes(svg_string=svg, width=s, height=s)))).convert("RGBA") for s in SIZES]
frames[-1].save(DIR / "codeyard.ico", sizes=[(s, s) for s in SIZES], append_images=frames[:-1])
print("wrote", DIR / "codeyard.ico")
