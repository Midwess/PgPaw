"""Post-process the AI-generated mascot JPEG into a 120x120 transparent PNG.

Strategy:
- Resize 1024x1024 -> 120x120 with Lanczos (high quality).
- BFS flood-fill from the four corners. Any pixel that is "white-ish"
  (min(R,G,B) >= threshold) and is reachable from a corner gets
  alpha=0. This correctly handles the drop shadow gradient: even a
  240-gray shadow next to a 255-white background gets stripped, but
  interior white highlights (tusk tips, eye bulbs) survive because
  they are surrounded by colored pixels and are not reachable from
  the corners.
"""

from collections import deque
from PIL import Image

SRC = "avatar_ai.jpeg"
OUT = "avatar.png"
SIZE = 160
THRESHOLD = 245


def flood_alpha_key(image: Image.Image) -> Image.Image:
    rgba = image.convert("RGBA")
    width, height = rgba.size
    pixels = rgba.load()
    visited = [[False] * height for _ in range(width)]
    queue: deque[tuple[int, int]] = deque()

    seeds = [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ]
    for sx, sy in seeds:
        r, g, b, _ = pixels[sx, sy]
        if min(r, g, b) >= THRESHOLD:
            visited[sx][sy] = True
            queue.append((sx, sy))

    while queue:
        x, y = queue.popleft()
        r, g, b, _ = pixels[x, y]
        pixels[x, y] = (r, g, b, 0)
        for dx, dy in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            nx, ny = x + dx, y + dy
            if 0 <= nx < width and 0 <= ny < height and not visited[nx][ny]:
                nr, ng, nb, _ = pixels[nx, ny]
                if min(nr, ng, nb) >= THRESHOLD:
                    visited[nx][ny] = True
                    queue.append((nx, ny))

    return rgba


def main() -> None:
    image = Image.open(SRC)
    print(f"source: {image.size} {image.mode}")

    resized = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    transparent = flood_alpha_key(resized)
    transparent.save(OUT, format="PNG", optimize=True)

    out = Image.open(OUT)
    print(f"wrote {OUT}  size={out.size}  mode={out.mode}")
    for (x, y) in [(0, 0), (119, 0), (0, 119), (119, 119), (60, 60), (60, 30)]:
        print(f"  ({x:3},{y:3}) = {out.getpixel((x, y))}")


if __name__ == "__main__":
    main()
