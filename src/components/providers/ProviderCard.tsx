import { useMemo, useState, useEffect } from "react";
import { GripVertical, ChevronDown, ChevronUp } from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  DraggableAttributes,
  DraggableSyntheticListeners,
} from "@dnd-kit/core";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import { cn } from "@/lib/utils";
import { ProviderIconBox } from "@/components/ProviderIconBox";
import { HoverTip } from "@/components/ui/hover-tip";
import UsageFooter from "@/components/UsageFooter";
import { extractProviderBaseUrl } from "@/utils/providerConfigUtils";
import { useProviderHealth } from "@/lib/query/failover";
import { useUsageQuery } from "@/lib/query/queries";
import { resolveProviderIcon } from "@/utils/providerIcon";
import { ProviderCardActions } from "./ProviderCardActions";
import type { CardChip, CardPresentation, CardTone } from "./presentation";

interface DragHandleProps {
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners;
  isDragging: boolean;
}

interface ProviderCardProps {
  provider: Provider;
  appId: AppId;
  /** 卡片怎么画：模式色、状态 / 按钮、徽标（由列表按模式算好） */
  presentation: CardPresentation;
  /** 当前激活的供应商（保留给列表与用量自动刷新使用） */
  isCurrent?: boolean;
  onEdit: (provider: Provider) => void;
  onDelete: (provider: Provider) => void;
  onConfigureUsage: (provider: Provider) => void;
  onOpenWebsite: (url: string) => void;
  onDuplicate?: (provider: Provider) => void;
  onTest?: (provider: Provider) => void;
  isTesting?: boolean;
  dragHandleProps?: DragHandleProps;
}

/** 当前那张：模式色边框 + 淡底。 */
const TONE_CLASS: Record<CardTone, string> = {
  direct: "border-direct-border bg-direct-soft",
  route: "border-route-border bg-route-soft",
  stack: "border-stack-border bg-stack-soft",
};

const CHIP_CLASS: Record<CardChip["tone"], string> = {
  outline: "border border-border-strong text-fg-2",
  direct: "bg-direct-soft text-direct-text",
  route: "bg-route-soft text-route-text",
  stack: "bg-stack-soft text-stack-text",
  success: "bg-success-soft text-success-text",
  warning: "bg-warning-soft text-warning-text",
  danger: "bg-danger-soft text-danger-text",
};

export function CardChipBadge({ chip }: { chip: CardChip }) {
  return (
    <span
      title={chip.title}
      className={cn(
        "inline-flex h-[18px] shrink-0 items-center whitespace-nowrap rounded-full px-1.5 text-badge",
        CHIP_CLASS[chip.tone],
        chip.title && "cursor-help",
      )}
    >
      {chip.label}
    </span>
  );
}

function HealthChip({
  consecutiveFailures,
  isHealthy,
}: {
  consecutiveFailures: number;
  isHealthy?: boolean;
}) {
  const { t } = useTranslation();
  const chip: CardChip =
    consecutiveFailures === 0
      ? { key: "health", label: t("health.operational"), tone: "success" }
      : isHealthy !== false
        ? { key: "health", label: t("health.degraded"), tone: "warning" }
        : { key: "health", label: t("health.circuitOpen"), tone: "danger" };
  return <CardChipBadge chip={chip} />;
}

const extractApiUrl = (provider: Provider, fallbackText: string) => {
  if (provider.notes?.trim()) {
    return provider.notes.trim();
  }

  if (provider.websiteUrl) {
    return provider.websiteUrl;
  }

  return extractProviderBaseUrl(provider.settingsConfig) ?? fallbackText;
};

export function ProviderCard({
  provider,
  appId,
  presentation,
  onEdit,
  onDelete,
  onConfigureUsage,
  onOpenWebsite,
  onDuplicate,
  onTest,
  isTesting,
  dragHandleProps,
}: ProviderCardProps) {
  const { t } = useTranslation();

  const { data: health } = useProviderHealth(
    provider.id,
    appId,
    Boolean(presentation.showHealth),
  );

  const fallbackUrlText = t("provider.notConfigured", {
    defaultValue: "未配置接口地址",
  });

  const displayUrl = useMemo(() => {
    return extractApiUrl(provider, fallbackUrlText);
  }, [provider, fallbackUrlText]);

  const isClickableUrl = useMemo(() => {
    if (provider.notes?.trim()) {
      return false;
    }
    if (displayUrl === fallbackUrlText) {
      return false;
    }
    return true;
  }, [provider.notes, displayUrl, fallbackUrlText]);

  const usageEnabled = provider.meta?.usage_script?.enabled ?? false;
  const autoQueryInterval = provider.meta?.usage_script?.autoQueryInterval ?? 0;
  const { data: usage } = useUsageQuery(provider.id, appId, {
    enabled: usageEnabled,
    autoQueryInterval,
  });

  const hasMultiplePlans =
    usageEnabled && usage?.success && usage.data && usage.data.length > 1;

  const [isExpanded, setIsExpanded] = useState(false);
  const expandLabel = isExpanded
    ? t("usage.collapse", { defaultValue: "收起" })
    : t("usage.expand", { defaultValue: "展开" });

  useEffect(() => {
    if (hasMultiplePlans) {
      setIsExpanded(true);
    }
  }, [hasMultiplePlans]);

  const handleOpenWebsite = () => {
    if (!isClickableUrl) {
      return;
    }
    onOpenWebsite(displayUrl);
  };

  return (
    <div
      className={cn(
        "group relative rounded-panel border transition-[border-color,box-shadow,background-color] duration-150",
        presentation.tone
          ? TONE_CLASS[presentation.tone]
          : "border-border bg-surface hover:border-border-strong hover:shadow-v7-sm",
        dragHandleProps?.isDragging && "cursor-grabbing shadow-v7-md",
      )}
    >
      <div className="flex min-h-[58px] items-center gap-3 py-2.5 pe-3 ps-2">
        {dragHandleProps ? (
          <button
            type="button"
            className={cn(
              "flex h-6 w-4 shrink-0 cursor-grab items-center justify-center text-fg-3 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 active:cursor-grabbing",
              dragHandleProps.isDragging && "cursor-grabbing opacity-100",
            )}
            aria-label={t("provider.dragHandle")}
            {...dragHandleProps.attributes}
            {...dragHandleProps.listeners}
          >
            <GripVertical className="h-4 w-4" />
          </button>
        ) : (
          <span className="w-4 shrink-0" />
        )}

        <ProviderIconBox
          icon={resolveProviderIcon(appId, provider.icon, provider.iconColor)}
          name={provider.name}
          color={provider.iconColor}
          className={cn(
            // 底色固定为白，图标颜色也要固定：单色（currentColor）图标和首字母
            // fallback 不能继承深色模式下的浅色文字
            "bg-white text-neutral-900",
            presentation.dim && "opacity-60",
          )}
          fallbackClassName="bg-transparent text-neutral-600"
        />

        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-1.5">
            <h3
              className={cn(
                "m-0 min-w-0 truncate text-strong",
                presentation.dim ? "text-fg-2" : "text-fg-1",
              )}
              title={provider.name}
            >
              {provider.name}
            </h3>
            {presentation.chips.map((chip) => (
              <CardChipBadge key={chip.key} chip={chip} />
            ))}
            {presentation.showHealth && health && (
              <HealthChip
                consecutiveFailures={health.consecutive_failures}
                isHealthy={health.is_healthy}
              />
            )}
          </div>

          {displayUrl ? (
            <button
              type="button"
              onClick={handleOpenWebsite}
              className={cn(
                "mt-0.5 inline-flex max-w-full items-center overflow-hidden text-start text-caption text-fg-2",
                isClickableUrl
                  ? "cursor-pointer decoration-border-strong underline-offset-[3px] hover:underline"
                  : "cursor-default",
              )}
              title={displayUrl}
              disabled={!isClickableUrl}
            >
              <span className="min-w-0 truncate">{displayUrl}</span>
            </button>
          ) : null}
        </div>

        <div className="flex shrink-0 items-center gap-3">
          {/* 额度列 136 宽，带重置倒计时时 194 宽（见 QuotaLines） */}
          <div className="max-w-[208px] text-end">
            <div className="flex items-center justify-end gap-1">
              {hasMultiplePlans ? (
                <span className="text-caption text-fg-2">
                  {t("usage.multiplePlans", {
                    count: usage?.data?.length || 0,
                    defaultValue: `${usage?.data?.length || 0} 个套餐`,
                  })}
                </span>
              ) : (
                <UsageFooter
                  provider={provider}
                  providerId={provider.id}
                  appId={appId}
                  usageEnabled={usageEnabled}
                  inline={true}
                />
              )}
              {hasMultiplePlans && (
                <HoverTip content={expandLabel}>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      setIsExpanded(!isExpanded);
                    }}
                    className="shrink-0 rounded-control p-1 text-fg-3 transition-colors hover:bg-subtle hover:text-fg-1"
                    aria-label={expandLabel}
                    aria-expanded={isExpanded}
                  >
                    {isExpanded ? (
                      <ChevronUp size={14} />
                    ) : (
                      <ChevronDown size={14} />
                    )}
                  </button>
                </HoverTip>
              )}
            </div>
          </div>
          <ProviderCardActions
            providerName={provider.name}
            presentation={presentation}
            onEdit={() => onEdit(provider)}
            onDelete={() => onDelete(provider)}
            onDuplicate={onDuplicate ? () => onDuplicate(provider) : undefined}
            onTest={onTest ? () => onTest(provider) : undefined}
            isTesting={isTesting}
            onConfigureUsage={() => onConfigureUsage(provider)}
          />
        </div>
      </div>

      {isExpanded && hasMultiplePlans && (
        <div className="mx-4 mb-3 border-t border-border pt-3">
          <UsageFooter
            provider={provider}
            providerId={provider.id}
            appId={appId}
            usageEnabled={usageEnabled}
            inline={false}
          />
        </div>
      )}
    </div>
  );
}
