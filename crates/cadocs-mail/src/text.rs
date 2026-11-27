//! Every text of a message, in the languages mail is written in (step 62). A placeholder is a
//! name in braces, which `fill` replaces with its value.

/// A language mail is written in: those of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
}

impl Lang {
    /// The language of a code as stored (`users.language`, `workspaces.language`), English for
    /// none or one mail is not written in.
    pub fn of(code: Option<&str>) -> Self {
        match code {
            Some("fr") => Self::Fr,
            _ => Self::En,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
        }
    }

    /// The language of an `Accept-Language` header that mail is written in with the highest
    /// weight, the first one on a tie, or `None` when the header names none.
    pub fn from_accept_language(header: &str) -> Option<Self> {
        let mut best: Option<(Self, f32)> = None;
        for item in header.split(',') {
            let mut parts = item.split(';');
            let tag = parts.next().unwrap_or_default().trim();
            let primary = tag.split('-').next().unwrap_or_default();
            let lang = if primary.eq_ignore_ascii_case("fr") {
                Self::Fr
            } else if primary.eq_ignore_ascii_case("en") {
                Self::En
            } else {
                continue;
            };
            let weight = match parts.find_map(|part| part.trim().strip_prefix("q=")) {
                Some(q) => q.parse().unwrap_or(0.0),
                None => 1.0,
            };
            if weight > 0.0 && best.is_none_or(|(_, top)| weight > top) {
                best = Some((lang, weight));
            }
        }
        best.map(|(lang, _)| lang)
    }
}

/// The catalogue: a French text cannot be left out, and a test checks none is left in English.
macro_rules! catalogue {
    ($($name:ident: $en:expr, $fr:expr;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Text {
            $($name),*
        }

        impl Text {
            pub const ALL: &[Self] = &[$(Self::$name),*];

            pub fn get(self, lang: Lang) -> &'static str {
                match (self, lang) {
                    $((Self::$name, Lang::En) => $en,
                      (Self::$name, Lang::Fr) => $fr,)*
                }
            }
        }
    };
}

catalogue! {
    LoginCodeSubject: "Your cadocs login code", "Votre code de connexion cadocs";
    CodeSubject: "Your cadocs code", "Votre code cadocs";
    LoginCode:
        "Your code to log in to\n\
         {origin}\n\n\
         \x20   {code}\n\n\
         It works for 10 minutes.\n\
         If you did not just log in, someone knows your password: change\n\
         it.\n",
        "Votre code pour vous connecter à\n\
         {origin}\n\n\
         \x20   {code}\n\n\
         Il est valable 10 minutes.\n\
         Si vous ne venez pas de vous connecter, quelqu'un connaît votre\n\
         mot de passe : changez-le.\n";
    SecondFactorCode:
        "Your code to turn on the second factor of your account on\n\
         {origin}\n\n\
         \x20   {code}\n\n\
         It works for 10 minutes.\n\
         If you did not ask for it, ignore this message: nothing changes\n\
         without the code.\n",
        "Votre code pour activer le second facteur de votre compte sur\n\
         {origin}\n\n\
         \x20   {code}\n\n\
         Il est valable 10 minutes.\n\
         Si vous ne l'avez pas demandé, ignorez ce message : rien ne\n\
         change sans le code.\n";
    AccountSubject: "Your cadocs account", "Votre compte cadocs";
    MoveTaken:
        "Someone asked to move another account on\n\
         {origin}\n\
         to this address, which already has an account. Nothing has\n\
         changed.\n",
        "Quelqu'un a demandé de déplacer un autre compte de\n\
         {origin}\n\
         vers cette adresse, qui a déjà un compte. Rien n'a changé.\n";
    MoveCode:
        "Your code to move your account on\n\
         {origin}\n\
         to this address\n\n\
         \x20   {code}\n\n\
         It works for 10 minutes. If you did not ask for it, ignore this\n\
         message: nothing changes without the code.\n",
        "Votre code pour déplacer votre compte de\n\
         {origin}\n\
         vers cette adresse\n\n\
         \x20   {code}\n\n\
         Il est valable 10 minutes. Si vous ne l'avez pas demandé, ignorez\n\
         ce message : rien ne change sans le code.\n";
    AddressChangedSubject: "Your cadocs address changed", "Votre adresse cadocs a changé";
    AddressChanged:
        "Your account on\n\
         {origin}\n\
         now uses the address\n\
         {email}\n\
         If you did not ask for it, someone else holds your account: ask\n\
         an admin of the instance to get it back.\n",
        "Votre compte sur\n\
         {origin}\n\
         utilise désormais l'adresse\n\
         {email}\n\
         Si vous ne l'avez pas demandé, quelqu'un d'autre détient votre\n\
         compte : demandez à un administrateur de l'instance de le\n\
         récupérer.\n";
    ResetSubject: "Reset your cadocs password", "Réinitialisez votre mot de passe cadocs";
    Reset:
        "Someone asked to reset the password of your account on\n\
         {origin}\n\n\
         To choose a new password, open this link within one hour:\n\n\
         {link}\n\n\
         If you did not ask for this, ignore this message: your password\n\
         has not changed.\n",
        "Quelqu'un a demandé à réinitialiser le mot de passe de votre\n\
         compte sur\n\
         {origin}\n\n\
         Pour choisir un nouveau mot de passe, ouvrez ce lien dans l'heure :\n\n\
         {link}\n\n\
         Si vous ne l'avez pas demandé, ignorez ce message : votre mot de\n\
         passe n'a pas changé.\n";
    RegisterTaken:
        "Someone asked to create an account on\n\
         {origin}\n\
         with this address, which already has an account. Nothing has\n\
         changed.\n\n\
         To log in: {origin}/login\n\
         If you forgot your password: {origin}/forgot-password\n",
        "Quelqu'un a demandé à créer un compte sur\n\
         {origin}\n\
         avec cette adresse, qui a déjà un compte. Rien n'a changé.\n\n\
         Pour vous connecter : {origin}/login\n\
         Si vous avez oublié votre mot de passe : {origin}/forgot-password\n";
    RegisterCode:
        "Your code to create an account on\n\
         {origin}\n\n\
         \x20   {code}\n\n\
         It works for 10 minutes. If you did not ask for it, ignore this\n\
         message: no account is made without the code.\n",
        "Votre code pour créer un compte sur\n\
         {origin}\n\n\
         \x20   {code}\n\n\
         Il est valable 10 minutes. Si vous ne l'avez pas demandé, ignorez\n\
         ce message : aucun compte n'est créé sans le code.\n";
    NotificationSubject: "1 new notification on cadocs", "1 nouvelle notification sur cadocs";
    NotificationsSubject:
        "{count} new notifications on cadocs",
        "{count} nouvelles notifications sur cadocs";
    Hello: "Hello {name},\n\n", "Bonjour {name},\n\n";
    MentionLine: "{who} mentioned you in {title}\n", "{who} vous a mentionné dans {title}\n";
    ReplyLine: "{who} replied to you on {title}\n", "{who} vous a répondu sur {title}\n";
    CommentLine: "{who} commented on {title}\n", "{who} a commenté {title}\n";
    Someone: "Someone", "Quelqu'un";
    UntitledPage: "an untitled page", "une page sans titre";
    More: "And {count} more.\n\n", "Et {count} de plus.\n\n";
    NotificationsFooter:
        "You get this because notifications by mail are on in your account.\n",
        "Vous recevez ce message car les notifications par e-mail sont\n\
         activées dans votre compte.\n";
}

impl Text {
    /// The text in `lang`, each placeholder named in `values` replaced. A value is not read again,
    /// so a page title with braces stays as it is.
    pub fn fill(self, lang: Lang, values: &[(&str, &str)]) -> String {
        let mut rest = self.get(lang);
        let mut out = String::with_capacity(rest.len());
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let found = after.find('}').and_then(|end| {
                let (_, value) = values.iter().find(|(name, _)| *name == &after[..end])?;
                Some((end, value))
            });
            match found {
                Some((end, value)) => {
                    out.push_str(value);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}
