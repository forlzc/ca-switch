import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CommonConfigEditor } from "@/components/providers/forms/CommonConfigEditor";

vi.mock("@/components/JsonEditor", () => ({
  default: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (value: string) => void;
  }) => (
    <textarea
      aria-label="settings-json-editor"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

describe("CommonConfigEditor", () => {
  it("forwards JSON edits to the form", () => {
    const onChange = vi.fn();
    render(
      <CommonConfigEditor
        value={JSON.stringify({ env: { ANTHROPIC_BASE_URL: "https://a.example" } })}
        onChange={onChange}
      />,
    );

    fireEvent.change(screen.getByLabelText("settings-json-editor"), {
      target: { value: "{}" },
    });

    expect(onChange).toHaveBeenCalledWith("{}");
  });
});
