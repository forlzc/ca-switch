import { Fragment } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown } from "lucide-react";
import { FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { HelpTip } from "@/components/ui/help-tip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { isValidUserAgentHeader } from "@/lib/userAgent";
import { cn } from "@/lib/utils";
import {
  USER_AGENT_PRESETS,
  type UserAgentPresetClient,
} from "@/config/userAgentPresets";

interface CustomUserAgentFieldProps {
  /** 输入框的 id（用于 label htmlFor）。 */
  id: string;
  value: string;
  onChange: (value: string) => void;
  /** 字段名后「?」里的说明。 */
  help: string;
}

/** 预设下拉的分组顺序；空组自动跳过。 */
const PRESET_GROUP_ORDER: readonly UserAgentPresetClient[] = [
  "claude-code",
  "codex",
  "other",
];

/**
 * 供应商级自定义 User-Agent 字段。
 *
 * 含标签 + 输入框 + 右侧预设下拉菜单 + 实时合法性提示。预设按客户端分组
 * （Claude Code / Codex / 其他），当前值在菜单里打勾。校验口径与后端
 * `parse_custom_user_agent` 一致（见 `@/lib/userAgent`），非法时给非阻断红字提示
 * （运行时仍会静默忽略）。
 */
export function CustomUserAgentField({
  id,
  value,
  onChange,
  help,
}: CustomUserAgentFieldProps) {
  const { t } = useTranslation();
  const valid = isValidUserAgentHeader(value);
  const label = t("providerForm.customUserAgent", {
    defaultValue: "自定义 User-Agent",
  });

  const groupLabels: Record<UserAgentPresetClient, string> = {
    "claude-code": t("providerForm.customUserAgentGroupClaudeCode", {
      defaultValue: "Claude Code",
    }),
    codex: t("providerForm.customUserAgentGroupCodex", {
      defaultValue: "Codex",
    }),
    other: t("providerForm.customUserAgentGroupOther", {
      defaultValue: "其他",
    }),
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-0.5">
        <FormLabel htmlFor={id}>{label}</FormLabel>
        <HelpTip title={label}>{help}</HelpTip>
      </div>
      <div className="flex items-center gap-2">
        <Input
          id={id}
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="Mozilla/5.0 ..."
          autoComplete="off"
          className="flex-1"
        />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" className="shrink-0 gap-1">
              {t("providerForm.customUserAgentPresets", {
                defaultValue: "预设",
              })}
              <ChevronDown className="h-3.5 w-3.5 opacity-60" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            className="max-h-72 w-[340px] overflow-y-auto z-[200]"
          >
            {PRESET_GROUP_ORDER.map((client) => {
              const presets = USER_AGENT_PRESETS.filter(
                (preset) => preset.client === client,
              );
              if (presets.length === 0) return null;
              return (
                <Fragment key={client}>
                  <DropdownMenuLabel className="text-xs font-medium text-fg-2">
                    {groupLabels[client]}
                  </DropdownMenuLabel>
                  {presets.map((preset) => {
                    const selected = preset.value === value;
                    return (
                      <DropdownMenuItem
                        key={`${preset.client}:${preset.value}`}
                        onSelect={() => onChange(preset.value)}
                        className="gap-2 font-mono text-xs"
                      >
                        <Check
                          className={cn(
                            "h-3.5 w-3.5 shrink-0",
                            selected ? "opacity-100" : "opacity-0",
                          )}
                        />
                        <span className="break-all">{preset.value}</span>
                      </DropdownMenuItem>
                    );
                  })}
                </Fragment>
              );
            })}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {!valid && (
        <p className="text-xs text-destructive">
          {t("providerForm.customUserAgentInvalid", {
            defaultValue:
              "User-Agent 不能包含控制字符（如换行符），否则将被忽略。",
          })}
        </p>
      )}
    </div>
  );
}
