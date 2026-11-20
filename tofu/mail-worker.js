function kindOf(envelopeTo) {
  const at = envelopeTo.lastIndexOf("@");
  const local = at < 0 ? envelopeTo : envelopeTo.slice(0, at);
  return local.toLowerCase() === "hook" ? "hook" : "throwaway";
}

function decodeWords(value) {
  return value
    .replace(/(\?=)\s+(?==\?)/g, "$1")
    .replace(/=\?([^?\s]+)\?([BbQq])\?([^?\s]*)\?=/g, (word, charset, encoding, text) => {
      try {
        const binary =
          encoding.toUpperCase() === "B"
            ? atob(text)
            : text
                .replace(/_/g, " ")
                .replace(/=([0-9A-Fa-f]{2})/g, (_, hex) => String.fromCharCode(parseInt(hex, 16)));
        const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
        return new TextDecoder(charset.split("*")[0]).decode(bytes);
      } catch {
        return word;
      }
    });
}

function eventOf(kind, message) {
  const header = (name) => decodeWords(message.headers.get(name) ?? "");
  return {
    kind,
    from: header("from") || message.from,
    to: kind === "throwaway" ? message.to : header("to") || message.to,
    subject: header("subject"),
  };
}

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
      const response = await fetch(env.HOOK_URL, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${env.HOOK_TOKEN}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(eventOf(kind, message)),
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
