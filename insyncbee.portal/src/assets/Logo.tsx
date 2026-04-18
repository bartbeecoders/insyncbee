export function LogoMark({
  size = 32,
  className,
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 64 64"
      aria-hidden="true"
    >
      <defs>
        <linearGradient id="lm-grad" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#f5a623" />
          <stop offset="100%" stopColor="#d27b00" />
        </linearGradient>
      </defs>
      <polygon
        points="32,3 58,17 58,47 32,61 6,47 6,17"
        fill="url(#lm-grad)"
      />
      <text
        x="32"
        y="42"
        textAnchor="middle"
        fontFamily="Inter, sans-serif"
        fontWeight="800"
        fontSize="34"
        fill="#0f1115"
      >
        B
      </text>
    </svg>
  );
}

/** Decorative honeycomb cluster for the hero. */
export function Honeycomb({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 360 360"
      aria-hidden="true"
    >
      <defs>
        <linearGradient id="hc-a" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#f5a623" />
          <stop offset="100%" stopColor="#b26600" />
        </linearGradient>
        <linearGradient id="hc-b" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#2a2e38" />
          <stop offset="100%" stopColor="#1a1d24" />
        </linearGradient>
        <linearGradient id="hc-accent" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#ffcc5a" />
          <stop offset="100%" stopColor="#f5a623" />
        </linearGradient>
      </defs>
      {/* Hex ring around a central hex */}
      {HexGrid.map((h, i) => (
        <polygon
          key={i}
          points={hexPath(h.cx, h.cy, 48)}
          fill={h.kind === "accent" ? "url(#hc-accent)" : h.kind === "primary" ? "url(#hc-a)" : "url(#hc-b)"}
          stroke="#3a404d"
          strokeWidth={h.kind === "accent" ? 0 : 1}
          opacity={h.kind === "dim" ? 0.7 : 1}
        />
      ))}
      {/* Bee */}
      <g transform="translate(180 180)">
        <ellipse cx="0" cy="0" rx="38" ry="26" fill="#0f1115" stroke="#f5a623" strokeWidth="2" />
        <rect x="-28" y="-10" width="14" height="20" fill="#f5a623" />
        <rect x="-2"  y="-10" width="14" height="20" fill="#f5a623" />
        <circle cx="-30" cy="-4" r="5" fill="#1a1d24" stroke="#ffcc5a" strokeWidth="1.5" />
        <path d="M 10 -18 Q 28 -34 18 -44 Q 2 -42 10 -24 Z" fill="#e4e6eb" opacity="0.7" />
        <path d="M 22 -10 Q 46 -22 42 -38 Q 20 -30 22 -14 Z" fill="#e4e6eb" opacity="0.6" />
      </g>
    </svg>
  );
}

type HexCell = { cx: number; cy: number; kind: "primary" | "dim" | "accent" };

const HexGrid: HexCell[] = (() => {
  // Centered axial coordinates for a 7-hex flower + outer ring fragments
  const r = 48;
  const dx = r * Math.sqrt(3);
  const dy = r * 1.5;
  const cx0 = 180;
  const cy0 = 180;
  const cells: HexCell[] = [];

  // Row-based honeycomb: 3 rows of 3/4/3 with accents
  const layout: Array<[number, number, HexCell["kind"]]> = [
    [-1, -1, "primary"], [0, -1, "dim"], [1, -1, "primary"],
    [-1.5, 0, "dim"], [-0.5, 0, "primary"], [0.5, 0, "accent"], [1.5, 0, "dim"],
    [-1, 1, "primary"], [0, 1, "dim"], [1, 1, "primary"],
  ];

  for (const [q, r2, kind] of layout) {
    cells.push({
      cx: cx0 + q * dx,
      cy: cy0 + r2 * dy,
      kind,
    });
  }
  return cells;
})();

function hexPath(cx: number, cy: number, size: number): string {
  const pts: string[] = [];
  for (let i = 0; i < 6; i++) {
    const angle = (Math.PI / 3) * i - Math.PI / 2;
    const x = cx + size * Math.cos(angle);
    const y = cy + size * Math.sin(angle);
    pts.push(`${x.toFixed(1)},${y.toFixed(1)}`);
  }
  return pts.join(" ");
}
