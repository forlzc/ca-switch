import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { HoverTip } from "@/components/ui/hover-tip";
import { Input } from "@/components/ui/input";
import { Download, Loader2, RefreshCw } from "lucide-react";
import type { FetchedModel } from "@/lib/api/model-fetch";
import { ModelDropdown } from "./ModelDropdown";

interface ModelInputWithFetchProps {
  id: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  fetchedModels: FetchedModel[];
  isLoading: boolean;
  /** 传入时显示获取/刷新按钮；不传时只在有数据后显示下拉 */
  onFetch?: () => void;
}

export function ModelInputWithFetch({
  id,
  value,
  onChange,
  placeholder,
  fetchedModels,
  isLoading,
  onFetch,
}: ModelInputWithFetchProps) {
  const { t } = useTranslation();
  const hasModels = fetchedModels.length > 0;

  // 传入 onFetch：按钮常驻，获取过一次后变成「刷新」，允许重复拉取
  if (onFetch) {
    const label = hasModels
      ? t("providerForm.refreshModels", { defaultValue: "刷新模型" })
      : t("providerForm.fetchModels", { defaultValue: "获取模型" });
    return (
      <div className="flex gap-1">
        <Input
          id={id}
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          autoComplete="off"
          className="flex-1"
        />
        {hasModels && (
          <ModelDropdown models={fetchedModels} onSelect={onChange} />
        )}
        <HoverTip content={label}>
          <Button
            aria-label={label}
            variant="outline"
            size="icon"
            className="shrink-0"
            type="button"
            disabled={isLoading}
            onClick={onFetch}
          >
            {isLoading ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : hasModels ? (
              <RefreshCw className="h-4 w-4" />
            ) : (
              <Download className="h-4 w-4" />
            )}
          </Button>
        </HoverTip>
      </div>
    );
  }

  // 没有 onFetch，但已有数据：Input + 下拉
  if (hasModels) {
    return (
      <div className="flex gap-1">
        <Input
          id={id}
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          autoComplete="off"
          className="flex-1"
        />
        <ModelDropdown models={fetchedModels} onSelect={onChange} />
      </div>
    );
  }

  return (
    <Input
      id={id}
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      autoComplete="off"
    />
  );
}
