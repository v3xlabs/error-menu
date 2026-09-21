import "@scalar/api-reference/style.css";

import { createApiReference } from "@scalar/api-reference";
import { onSettled } from "solid-js";

export const DocumentationPage = () => {
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

  const keepElement = (element: HTMLDivElement): void => {
    referenceElement = element;
  };

  return <div ref={keepElement} />;
};
