import { Server } from "lucide-react";
import type { AppId } from "@/lib/api";
import { cn } from "@/lib/utils";

/**
 * 入口协议名：通用代理软件不再出现具体客户端名称，只按协议区分入口。
 */
export const APP_DISPLAY_NAME: Record<AppId, string> = {
  claude: "API",
};

interface AppGlyphProps {
  /** 图标边长：侧栏 16，页头 20 */
  size?: number;
  className?: string;
}

/** 供应商图标的通用占位：不再显示任何具体客户端的品牌图标。 */
export function AppGlyph({ size = 16, className }: AppGlyphProps) {
  return (
    <Server
      aria-hidden="true"
      className={cn("shrink-0 text-fg-2", className)}
      style={{ width: size, height: size }}
      strokeWidth={1.75}
    />
  );
}
