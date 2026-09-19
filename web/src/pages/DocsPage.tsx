import { createApiReference } from "@scalar/api-reference";
import "@scalar/api-reference/style.css";
import { onSettled } from "solid-js";

export const DocsPage = () => {
  let referenceElement: HTMLDivElement | undefined;

  onSettled(() => {
    if (referenceElement === undefined) {
      return;
    }

    const reference = createApiReference(referenceElement, {
      url: "/openapi.json",
      persistAuth: false,
    });

    return reference.destroy;
  });

  return <div ref={element => { referenceElement = element; }} />;
};
