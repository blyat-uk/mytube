import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, act } from "@testing-library/react";
import { ToastProvider, useToast, type ToastAction } from "./Toast";

let toast!: ReturnType<typeof useToast>;
function Grab() {
  toast = useToast();
  return null;
}

function renderToasts() {
  render(<ToastProvider><Grab /></ToastProvider>);
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("toasts", () => {
  it("shows a plain toast with no action and dismisses it on click", () => {
    renderToasts();
    act(() => toast.info("No new videos."));
    expect(screen.getAllByRole("button")).toHaveLength(1);
    fireEvent.click(screen.getByTitle("Dismiss"));
    expect(screen.queryByText("No new videos.")).toBeNull();
  });

  it("still expires a plain info toast after five seconds", () => {
    vi.useFakeTimers();
    renderToasts();
    act(() => toast.info("No new videos."));
    act(() => { vi.advanceTimersByTime(5000); });
    expect(screen.queryByText("No new videos.")).toBeNull();
  });

  it("runs the action and dismisses, and gives an actionable toast longer to live", () => {
    vi.useFakeTimers();
    renderToasts();
    const action: ToastAction = { label: "Download", onClick: vi.fn() };
    act(() => toast.info("MyTube 0.1.2 is available.", action));

    // Well past a plain info toast's lifetime, the button is still there to press.
    act(() => { vi.advanceTimersByTime(9000); });
    fireEvent.click(screen.getByRole("button", { name: "Download" }));
    expect(action.onClick).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("MyTube 0.1.2 is available.")).toBeNull();
  });

  it("dismisses an actionable toast from its body without acting", () => {
    renderToasts();
    const action: ToastAction = { label: "Download", onClick: vi.fn() };
    act(() => toast.info("MyTube 0.1.2 is available.", action));
    fireEvent.click(screen.getByTitle("Dismiss"));
    expect(action.onClick).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Download" })).toBeNull();
  });
});
