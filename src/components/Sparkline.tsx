type Series = { values: number[]; color: string };

type Props = {
  series: Series[];
  /** 期望显示的点数（不足时右侧对齐） */
  points?: number;
  height?: number;
};

function toPolyline(values: number[], total: number, height: number): string {
  const width = 300;
  const pad = 2;
  const usable = width - pad * 2;
  const offset = Math.max(0, total - values.length);
  return values
    .map((v, i) => {
      const x = pad + (usable * (offset + i)) / Math.max(1, total - 1);
      const y = pad + (height - pad * 2) * (1 - Math.min(100, Math.max(0, v)) / 100);
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}

/** 迷你折线（tile 底部 60 点 CPU/GPU） */
export function Sparkline({ series, points = 60, height = 34 }: Props) {
  return (
    <svg
      className="sparkline"
      viewBox={`0 0 300 ${height}`}
      preserveAspectRatio="none"
      height={height}
    >
      <line x1="0" y1={height - 2} x2="300" y2={height - 2} stroke="rgba(255,255,255,0.08)" strokeWidth="1" />
      {series.map((s, i) => (
        <polyline
          key={i}
          points={toPolyline(s.values, points, height)}
          fill="none"
          stroke={s.color}
          strokeWidth="1.4"
          strokeLinejoin="round"
          strokeLinecap="round"
          vectorEffect="non-scaling-stroke"
        />
      ))}
    </svg>
  );
}