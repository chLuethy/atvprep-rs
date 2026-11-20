import { omit, type ParentComponent } from "solid-js";
import type { JSX } from "@solidjs/web";

type Variant = "default" | "primary" | "quiet";

type ButtonProps = Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, "class"> & { variant?: Variant; class?: string };

/** The one button vocabulary: default, primary (one per surface), quiet (tertiary). */
export const Button: ParentComponent<ButtonProps> = (props) => {
  const rest = omit(props, "variant", "children", "type", "class");
  const variant = () => props.variant ?? "default";
  return (
    <button type={props.type ?? "button"} class={["btn", variant() === "default" ? "" : variant(), props.class]} {...rest}>
      {props.children}
    </button>
  );
};
