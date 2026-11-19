import PostalMime from "postal-mime";
import { kindOf, toEvent } from "./event.js";

export default {
  async email(message, env, ctx) {
    if (!env.HOOK_TOKEN || !env.FORWARD_TO) {
      throw new Error("HOOK_TOKEN and FORWARD_TO must be set");
    }

    const kind = kindOf(message.to);

    if (kind === "hook" && message.headers.get("x-mail-hook")) {
      return;
    }

    let forwardError;
    if (kind === "throwaway") {
      try {
        await message.forward(env.FORWARD_TO, new Headers({ "X-Mail-Hook": "forwarded" }));
      } catch (error) {
        forwardError = error;
      }
    }

    try {
      const parsed = await PostalMime.parse(message.raw);
      const response = await fetch(env.HOOK_URL, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${env.HOOK_TOKEN}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(toEvent(kind, message.to, message.from, parsed)),
      });
      if (!response.ok) {
        console.error(`hook responded with ${response.status}`);
      }
    } catch (error) {
      console.error("hook failed", error);
    }

    if (forwardError) {
      throw forwardError;
    }
  },
};
