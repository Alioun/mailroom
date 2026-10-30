# Prepare the Gmail forwarding route

The personal route forwards new non-Spam mail from Gmail to a dedicated mailbox.org address. Gmail archives its retained copy. Existing mail stays in Gmail for its separate backlog workflow. No Gmail filters are deployed.

This preparation guide changes no provider settings. Live setup and delivery checks require the complete personal build and separate owner approval described in [Personal-use readiness](personal-readiness.md#gates-before-real-daily-use). The current prototype has not passed that gate.

## Read or save the plan

1. Open **Setup and recovery** in the account workspace.
2. Expand **Gmail forwarding plan**.
3. Select **Save the preparation plan** to download the six planned stages in the current interface language.

The plan contains generic instructions. It asks for no addresses, verification links, passwords or account details. Saving it creates a browser-managed text file. It does not update account state or establish provider evidence.

To read the same English stages in the terminal, run:

```sh
./scripts/prepare-gmail-route.sh --plan
```

The terminal wizard prints a fixed preparation record and exits. It opens no browser, reads no account repository, captures no input and changes no settings. Unsupported modes, including `--run` and `--approve`, fail. There is no live mode in this version.

## Keep the live checks separate

The later approved owner procedure must verify Gmail's forwarding setting, destination verification and archived copy. It must separately establish mailbox.org delivery, existing-rule coexistence, the envelope recipient and persistent `$via-gmail` storage. Test sends belong to that later approval.

Existing mailbox.org rules win conflicting filing choices. Approved business accounting copies run separately before existing filing rules. Unknown or mixed recipient roles retain Inbox and read status without accounting copies. Verified ingress tagging can still apply under that hold. A header claim, setup checkbox or downloaded plan cannot install envelope authority. The route keyword does not authenticate Gmail origin.

The owner disables forwarding in Gmail settings to stop future forwarded deliveries during an approved procedure. This does not reverse earlier deliveries. Mailroom's Cancel stops new sorting batches; completed changes remain until a separate confirmed Undo.

The [Google forwarding instructions](https://support.google.com/mail/answer/10957?hl=en) and [mailbox.org Sieve instructions](https://kb.mailbox.org/en/private/e-mail/can-i-use-sieve-mail-filtering/) describe provider behavior. They do not prove this account's setup or delivery. The plan follows the cached first-party grounding; current account settings and provider interface acceptance remain unverified.

## Verify the preparation implementation

`ui/scripts/verify-gmail-route.mjs <output-directory>` runs the actual terminal plan with dummy commands, checks refusal paths, builds the actual Svelte component and compares its English/German downloads with the common plan model. It checks that preparation starts no provider, browser, credential or build commands. Server rendering does not establish mounted interaction or visual acceptance.
