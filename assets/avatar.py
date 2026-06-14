"""Render the PgPaw avatar: 120x120 transparent PNG.

Mascot is a hybrid creature:
- PG-blue elephant head (round face, two big floppy ears, descending trunk)
- Rust-orange Ferris-crab claws in front
- Rust eyestalks on top with white eyes + black pupils
- Subtle smile

Postgres blue  : #336791 (deep), #4A90C2 (mid)
Rust orange    : #CE422B (Ferris), #B7410E (deep)
"""

from PIL import Image, ImageDraw

SIZE = 120

PG_BLUE = (51, 103, 145, 255)
PG_BLUE_MID = (74, 144, 194, 255)
PG_BLUE_DEEP = (38, 78, 110, 255)
RUST = (206, 66, 43, 255)
RUST_DEEP = (183, 65, 14, 255)
WHITE = (255, 255, 255, 255)
BLACK = (20, 20, 20, 255)


def make() -> Image.Image:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # Ears (PG blue, behind head). Big floppy elephant ears.
    for cx, lean in [(28, -1), (92, 1)]:
        ear = [
            (cx - 4 * lean, 38),
            (cx - 16 * lean, 50),
            (cx - 18 * lean, 68),
            (cx - 12 * lean, 82),
            (cx - 2 * lean, 80),
            (cx + 6 * lean, 64),
            (cx + 4 * lean, 48),
        ]
        draw.polygon(ear, fill=PG_BLUE_MID)
    for cx, lean in [(30, -1), (90, 1)]:
        inner = [
            (cx - 2 * lean, 56),
            (cx - 8 * lean, 64),
            (cx - 6 * lean, 76),
            (cx + 2 * lean, 72),
            (cx + 4 * lean, 60),
        ]
        draw.polygon(inner, fill=PG_BLUE_DEEP)

    # Claws (rust), behind head, peeking out lower-left and lower-right
    draw_crab_claw(draw, cx=22, cy=86, scale=1.0, mirror=False)
    draw_crab_claw(draw, cx=98, cy=86, scale=1.0, mirror=True)

    # Head / body (round PG-blue circle)
    head_box = (28, 28, 92, 92)
    draw.ellipse(head_box, fill=PG_BLUE)

    # Soft top highlight
    draw.ellipse((36, 32, 76, 56), fill=PG_BLUE_MID)

    # Eyestalks (rust)
    for stalk_x in [50, 70]:
        draw.line(
            [(stalk_x, 30), (stalk_x, 18)],
            fill=RUST_DEEP,
            width=4,
        )

    # Eye bulbs (white) + pupils (black)
    for eye_x, eye_y in [(50, 18), (70, 18)]:
        draw.ellipse(
            (eye_x - 6, eye_y - 6, eye_x + 6, eye_y + 6),
            fill=WHITE,
            outline=RUST_DEEP,
            width=1,
        )
        draw.ellipse(
            (eye_x - 2, eye_y - 2, eye_x + 2, eye_y + 2),
            fill=BLACK,
        )

    # Trunk (PG blue descending curve)
    trunk = [
        (54, 76),
        (50, 88),
        (50, 100),
        (54, 110),
        (60, 112),
        (64, 108),
        (62, 100),
        (62, 88),
        (60, 76),
    ]
    draw.polygon(trunk, fill=PG_BLUE_DEEP)

    # Trunk tip highlight
    draw.ellipse((56, 108, 64, 114), fill=PG_BLUE)

    # Tusks (small white triangles flanking trunk base)
    draw.polygon([(46, 78), (50, 90), (52, 78)], fill=WHITE)
    draw.polygon([(74, 78), (70, 90), (68, 78)], fill=WHITE)

    # Smile (small dark arc)
    draw.arc(
        (50, 70, 70, 84),
        start=20,
        end=160,
        fill=BLACK,
        width=2,
    )

    # Claw front "fingers" on top of head, in front of face
    draw_crab_claw(draw, cx=30, cy=98, scale=0.85, mirror=False, front=True)
    draw_crab_claw(draw, cx=90, cy=98, scale=0.85, mirror=True, front=True)

    return img


def draw_crab_claw(
    draw: ImageDraw.ImageDraw,
    cx: int,
    cy: int,
    scale: float,
    mirror: bool,
    front: bool = False,
) -> None:
    """Draw a stylized Ferris-crab claw centered at (cx, cy).

    The claw is a fat crescent with a V-shaped pincer notch. When front=True
    the claw sits over the head (rust on PG blue); otherwise it sits behind
    the head as a deeper rust tone.
    """
    s = scale
    direction = -1 if mirror else 1
    color = RUST if front else RUST_DEEP

    body = [
        (cx - 18 * direction * s, cy - 14 * s),
        (cx - 22 * direction * s, cy),
        (cx - 16 * direction * s, cy + 14 * s),
        (cx - 2 * direction * s, cy + 16 * s),
        (cx + 6 * direction * s, cy + 8 * s),
        (cx + 4 * direction * s, cy - 2 * s),
        (cx + 2 * direction * s, cy - 10 * s),
    ]
    draw.polygon(body, fill=color)

    notch_color = PG_BLUE_DEEP if front else PG_BLUE
    notch = [
        (cx + 4 * direction * s, cy - 6 * s),
        (cx - 2 * direction * s, cy),
        (cx + 4 * direction * s, cy + 6 * s),
    ]
    draw.polygon(notch, fill=notch_color)

    if front:
        ex0 = min(cx - 10 * direction * s, cx - 2 * direction * s)
        ex1 = max(cx - 10 * direction * s, cx - 2 * direction * s)
        draw.ellipse(
            (ex0, cy - 8 * s, ex1, cy + 8 * s),
            fill=RUST_DEEP,
        )


def main() -> None:
    image = make()
    out = "assets/avatar.png"
    image.save(out, format="PNG", optimize=True)
    print(f"wrote {out}  size={image.size}  mode={image.mode}")


if __name__ == "__main__":
    main()
