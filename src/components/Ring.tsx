import { clamp } from "../lib/format";

type Props = {
  value: number;
  color: string;
  label: string;
  caption?: string;
  /** 环形半径（默认 30） */
  radius?: number;
  stroke?: number;
};

/** 自写 SVG 环形进度（无第三方图表库） */
export function Ring({ value, color, label, caption, radius = 32, stroke = 7 }: Props) {
  const size = (radius + stroke / 2) * 2;
  const center = size / 2;
  const circumference = 2 * Math.PI * radius;
  const pctValue = clamp(value, 0, 100);
  const offset = circumference * (1 - pctValue / 100);

  return (
    <div className="ring">
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-label={`${label} ${pctValue.toFixed(0)}%`}>
        <circle
          cx={center}
          cy={center}
          r={radius}
          fill="none"
          stroke="rgba(255,255,255,0.10)"
          strokeWidth={stroke}
        />
        <circle
          cx={center}
          cy={center}
          r={radius}
          fill="none"
          stroke={color}
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={offset}
          transform={`rotate(-90 ${center} ${center})`}
          style={{ transition: "stroke-dashoffset 400ms ease" }}
        />
        <text
          x={center}
          y={center + 1}
          textAnchor="middle"
          dominantBaseline="middle"
          className="ring-value"
          fill="#f0f6fc"
        >
          {pctValue.toFixed(0)}
        </text>
      </svg>
      <div className="ring-caption">
        <span style={{ color }}>{label}</span>
        {caption ? <span className="dim mono">{caption}</span> : null}
      </div>
    </div>
  );
}