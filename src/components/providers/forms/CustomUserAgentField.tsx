import { useTranslation } from "react-i18next";
import { FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { HelpTip } from "@/components/ui/help-tip";
import { isValidUserAgentHeader } from "@/lib/userAgent";

interface CustomUserAgentFieldProps {
  /** 输入框的 id（用于 label htmlFor）。 */
  id: string;
  value: string;
  onChange: (value: string) => void;
  /** 字段名后「?」里的说明。 */
  help: string;
}

/**
 * 供应商级自定义 User-Agent 字段。
 *
 * 只提供自由输入与合法性校验，不内置任何客户端 UA 预设。
 * 校验口径与后端 `parse_custom_user_agent` 一致（见 `@/lib/userAgent`），
 * 非法时给非阻断红字提示（运行时仍会静默忽略）。
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

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-0.5">
        <FormLabel htmlFor={id}>{label}</FormLabel>
        <HelpTip title={label}>{help}</HelpTip>
      </div>
      <Input
        id={id}
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="Mozilla/5.0 ..."
        autoComplete="off"
        className="w-full"
      />
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
