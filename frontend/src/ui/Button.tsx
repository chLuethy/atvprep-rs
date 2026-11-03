import { splitProps, type JSX, type ParentComponent } from "solid-js";

type Variant = "default" | "primary" | "quiet";

type ButtonProps = Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, "class"> & { variant?: Variant; class?: string };

/** The one button vocabulary: default, primary (one per surface), quiet (tertiary). */
export const Button: ParentComponent<ButtonProps> = (props) => {
  const [own, rest] = splitProps(props, ["variant", "children", "type", "class"]);
  const variant = () => own.variant ?? "default";
  return (
    <button type={own.type ?? "button"} class={["btn", variant() === "default" ? "" : variant(), own.class ?? ""].filter(Boolean).join(" ")} {...rest}>
      {own.children}
    </button>
  );
};
