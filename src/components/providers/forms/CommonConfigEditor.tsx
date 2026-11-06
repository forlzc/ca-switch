import { useTranslation } from "react-i18next";
import { useEffect, useState } from "react";
import { Label } from "@/components/ui/label";
import JsonEditor from "@/components/JsonEditor";
import type { ClaudeApiFormat } from "@/types";

interface CommonConfigEditorProps {
  value: string;
  onChange: (value: string) => void;
  apiFormat?: ClaudeApiFormat;
}

/** 配置 JSON 的占位示例跟随所选上游协议，不再是 Anthropic 专用。 */
const CONFIG_PLACEHOLDERS: Record<ClaudeApiFormat, string> = {
  anthropic: `{
  "env": {
    "ANTHROPIC_BASE_URL": "https://your-api-endpoint.com",
    "ANTHROPIC_AUTH_TOKEN": "your-api-key-here"
  }
}`,
  openai_chat: `{
  "env": {
    "OPENAI_BASE_URL": "https://api.openai.com/v1",
    "OPENAI_API_KEY": "sk-your-api-key"
  }
}`,
  openai_responses: `{
  "env": {
    "OPENAI_BASE_URL": "https://api.openai.com/v1",
    "OPENAI_API_KEY": "sk-your-api-key"
  }
}`,
  gemini_native: `{
  "env": {
    "GOOGLE_GEMINI_BASE_URL": "https://generativelanguage.googleapis.com",
    "GEMINI_API_KEY": "your-api-key"
  }
}`,
};

export function CommonConfigEditor({
  value,
  onChange,
  apiFormat = "anthropic",
}: CommonConfigEditorProps) {
  const { t } = useTranslation();
  const [isDarkMode, setIsDarkMode] = useState(false);

  useEffect(() => {
    setIsDarkMode(document.documentElement.classList.contains("dark"));

    const observer = new MutationObserver(() => {
      setIsDarkMode(document.documentElement.classList.contains("dark"));
    });

    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });

    return () => observer.disconnect();
  }, []);

  // Mirror value prop into local state so the editor stays in sync
  // (parent uses form.getValues which doesn't trigger re-renders)
  const [localValue, setLocalValue] = useState(value);

  useEffect(() => {
    setLocalValue(value);
  }, [value]);

  const handleLocalChange = (newValue: string) => {
    setLocalValue(newValue);
    onChange(newValue);
  };

  return (
    <div className="space-y-2">
      <Label htmlFor="settingsConfig">{t("provider.configJson")}</Label>
      <p className="text-xs text-fg-2">
        {t("provider.configJsonHint", {
          defaultValue: "以下为供应商的完整配置，保存后用于路由转发。",
        })}
      </p>
      <JsonEditor
        value={localValue}
        onChange={handleLocalChange}
        ariaLabel={t("provider.configJson")}
        placeholder={
          CONFIG_PLACEHOLDERS[apiFormat] ?? CONFIG_PLACEHOLDERS.anthropic
        }
        darkMode={isDarkMode}
        rows={3}
        showValidation={true}
        language="json"
      />
    </div>
  );
}
