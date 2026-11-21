# Kestrel — Especificação v0.1

- Código-fonte: `.ks` · Bytecode: `.ksb`
- Alvo: BET-VM, processador ternário balanceado de 27 trits
- Paradigma: imperativo estruturado, determinístico, sem GC, posse linear estrita
- Lógica: Kleene K₃ com propagação de veneno (NaT)

---

## 1. Dados

### 1.1 Trit e codificação BET

| Lógico | Símbolo | BET | Valor |
|---|---|---|---|
| Negativo / Falso | `-` | `10` | −1 |
| Neutro / Indeterminado | `0` | `00` | 0 |
| Positivo / Verdadeiro | `+` | `01` | +1 |
| Veneno (NaT) | `?` | `11` | ⊥ |

### 1.2 Tipos escalares (copiáveis)

| Tipo no código | Largura | Intervalo |
|---|---|---|
| `bool` | 1 trit | `true` (+1) ou `false` (−1) |
| `trit` | 1 trit | −1 .. +1 |
| `tryte` | 6 trits (t6) | −364 .. +364 |
| `int` | 27 trits (t27) | −3.812.798.742.493 .. +3.812.798.742.493 |
| `real` | 27 trits (t27) | ≈ ±10¹⁷⁴, com cerca de 10 dígitos decimais de precisão (§1.5) |
| `text` | referência | String literal imutável (§1.6) |
| enum declarado (`Cor`) | 27 trits (t27) | Um dos valores listados na declaração (§8.7) |

Neste documento, `t6` e `t27` indicam larguras. No código-fonte, os tipos se escrevem `tryte` e `int`.

**`bool` × `trit`:** o `bool` é o booleano comum (`true`/`false`), para quem não precisa da lógica ternária. O `trit` acrescenta o `0`, "desconhecido".

- As comparações (`== != < > <= >=`) devolvem `bool`. Só o `<=>` devolve `trit` (−1, 0 ou +1).
- `if`, `while` e `for` aceitam `bool` ou `trit` (com `trit`, o `0` dispara Trap).
- `&&`, `||` e `!` com dois `bool` dão `bool`. Com algum `trit`, seguem a lógica de Kleene (§3.1) e dão `trit`.
- Um `bool` cabe num `trit` sem cast (`t: trit = ok;`). O contrário exige `(bool) t`, que falha (Trap, ou NaT com `on NaT`) se `t` não for 1 ou −1. `(int) ok` dá 1 ou −1.
- `bool` aceita só `==` e `!=`; contas são erro de tipo. `print`, `{ok}`, `%s` e `toText` mostram `true`/`false`; `%d` exige cast.
- `isDigit` e cia., `fileExists` e `eof` devolvem `bool`. `claim` preenche arrays de `bool` com `false`.

### 1.3 NaT (veneno)

- O NaT vale para a **palavra inteira**. Basta um trit `?` para a palavra toda ser NaT.
- **Uma conta que falha para o programa (Trap)**, com uma mensagem que diz o motivo: overflow, divisão por zero, cast fora do intervalo, `toInt` de texto inválido. Como uma exceção não tratada no Java.
- **Exceto numa linha com `on NaT`** (§8.5). Ali a conta que falha produz NaT, qualquer operação com NaT também produz NaT (o veneno se espalha), e o bloco do `on NaT` trata o erro. Isso vale só para as contas escritas na própria linha: um proc chamado dela continua parando o programa.
- Um proc pode avisar que falhou de propósito com `return NaT;`, como retornar `null`. Quem chama trata com `on NaT`.
- Usar um valor NaT numa conta fora de uma linha com `on NaT` também dispara Trap.
- Na fonte, o NaT se escreve com a palavra-chave `NaT` ou com qualquer literal que contenha `?`.

### 1.4 Conversões

- A **ampliação** `trit → t6 → t27` é implícita e não perde informação.
- O **estreitamento** exige um *cast* no estilo C: `(tryte) x`, `(trit) x`. Um valor fora do intervalo é uma conta que falha (§1.3).
- A conversão entre `int` e `real` é **sempre explícita**. `(real) x` arredonda para o valor mais próximo, e `(int) r` trunca em direção a zero (fora do intervalo, falha).

### 1.5 `real` (ponto flutuante ternário)

```
int de 27 trits:  [ 6 trits expoente | 21 trits mantissa ]
valor = (mantissa / 3²⁰) × 3^expoente
```

- A mantissa é **normalizada**: o trit mais significativo é sempre `-` ou `+`, exceto no zero. Não há trit de sinal, porque o sinal está na própria mantissa.
- O expoente é um `tryte`, de −364 a +364.
- O arredondamento é sempre para o valor mais próximo, que é o efeito natural de descartar trits.
- Overflow e divisão por zero são contas que falham (§1.3). **Não existe infinito.** Um underflow vira 0.
- Frações de 3 são exatas: `1.0 / 3.0` não tem erro de arredondamento.

### 1.6 `text` (string)

- É um literal imutável guardado no pool de strings do `.ksb`, como o `const char*` do C. É copiável e **não precisa de `burn`**.
- `s[i]` devolve o `tryte` da posição `i`. **O índice começa em 0**, como em C (como os arrays sem `sym`). Índice fora de `0 .. len(s)-1` dispara **Trap**.
- `len(s)` devolve o tamanho como `int`.
- `a + b` cria um texto novo: `saudacao: text = "Olá, " + nome + "!";`. Só vale **texto + texto**: `"x = " + 5` é **erro de tipo** (use `toText(5)`).
- `a == b` e `a != b` comparam caractere por caractere. `<` e `>` não valem para textos.
- `sub(s, início, fim)` devolve os caracteres de `início` até `fim - 1`, como o `substring` do Java: `sub("Kestrel", 0, 3)` é `"Kes"`. Fora de `0 ≤ início ≤ fim ≤ len(s)` dispara **Trap**.
- `toInt(s)` e `toReal(s)` leem um número (`"123"`, `"-7"`, `"0t+0-"`, `"2.5"`). Texto inválido é uma conta que falha (§1.3): use `on NaT` para tratar.
- `isDigit(c)`, `isLetter(c)`, `isUpper(c)`, `isLower(c)` e `isSpace(c)` dizem (`+`/`-`) se um `tryte` é dígito, letra (com acentos), maiúscula, minúscula ou espaço (`' '`, `\n`, `\t`).
- Cada caractere precisa existir na tabela §4. Um caractere fora dela, como `&` ou `ñ`, é **erro de compilação**.
- `toText(x)` cria um texto novo a partir de um número (§8.6). Um texto criado com o programa rodando (`+`, `sub`, `toText`, `readLine`...) é liberado sozinho quando nenhuma variável, array ou valor em uso aponta mais para ele (coleta de lixo só para textos, sem pausas perceptíveis). O limite é de 2²⁴ caracteres em textos **vivos ao mesmo tempo**. Para montar texto caractere por caractere, use um array de `tryte`, como um array de `char` em C.

---

## 2. Literais

| Forma | Exemplo | Regra |
|---|---|---|
| Decimal | `42`, `-8` | Fora de ±3.812.798.742.493 é **erro de compilação** |
| Real | `3.14`, `-0.5`, `2.0` | Exige um dígito antes e depois do ponto, e tem o tipo `real` |
| Ternário `0t` | `0t+0-` (= 8) | 1 a 27 trits, sendo o mais significativo à esquerda |
| Heptavintimal `0v` | `0vH`, `0vm` | 1 a 9 dígitos: `0` = 0, `A`–`M` = +1..+13, `a`–`m` = −1..−13, `?` = NaT |
| Trit | `-`, `0`, `+` | Na posição de valor (argumentos, ramos do `tri`) |
| Caractere | `'A'`, `'a'`, `' '`, `'\n'` | Vale o `tryte` da tabela §4 (`'A'` = +1, `'a'` = −1, `' '` = 0) |
| Veneno | `NaT` | — |
| String | `"Hello\n"` | Tem o tipo `text`. Escapes: `\n` `\t` |

**Comentários:** `//` até o fim da linha.

Cada dígito `0v` vale 3 trits, então uma palavra `t27` cabe em 9 dígitos. Um literal `0t` termina no primeiro caractere que não é trit, de modo que `0t+-` seguido de `- y` precisa de espaço entre eles.

---

## 3. Operadores

Em ordem de precedência, da menor para a maior:

| Prec. | Operador | Semântica |
|---|---|---|
| 1 | `\|\|` | Disjunção de Kleene (`max`) |
| 2 | `&&` | Conjunção de Kleene (`min`) |
| 3 | `<=>` `==` `!=` `<` `>` `<=` `>=` | Comparações, resultado `trit`. `<=>` dá −, 0 ou +; as outras dão `+` (verdadeiro) ou `−` (falso). Não podem ser encadeadas: `a < b < c` é erro |
| 4 | `+` `-` | Soma e subtração |
| 5 | `*` `/` `%` | Multiplicação, divisão e resto |
| 6 | `<<` `>>` | Deslocamento ternário (×3ᵏ e ÷3ᵏ) |
| 7 | `!` `-` (unário), `(T)` | Negação lógica (`trit`), negação numérica e cast |
| 8 | `**` | Potência, associativa à direita (`2 ** 3 ** 2 = 2 ** 9`) |

Regras (onde se lê "produz NaT", fora de uma linha com `on NaT` a conta para o programa: §1.3):

- **Overflow** além da largura do tipo resultante produz **NaT**. Não existe wrap-around.
- **`/`** trunca em direção a zero. Divisão por zero produz NaT.
- **`%`** é o resto da divisão. O sinal segue o dividendo, e vale `a = (a/b)·b + a%b`. Resto por zero produz NaT.
- **`>>`** descarta os trits inferiores, o que equivale a arredondar para o inteiro mais próximo. Exemplo: `2 >> 1 = 1`, enquanto `2 / 3 = 0`.
- **`<<`** produz NaT se algum trit não nulo passar da largura do tipo. Com `k ≥ 27`, o resultado é `0` se o operando for 0 e NaT caso contrário. Com `k < 0` ou `k` NaT, o resultado é NaT.
- **`!x`** inverte um `trit` (`!+ = −`, `!0 = 0`). Sobre números é erro de tipo; use `-x`. Os dois compilam para o mesmo opcode `NEG`, porque em ternário balanceado negar é inverter cada trit.
- As **comparações** aceitam números do mesmo tipo (`int` com `real` exige cast) e devolvem `bool` (`<=>` devolve `trit`). Se um lado for NaT, o resultado é NaT. Textos, enums e `bool` aceitam só `==` e `!=`.
- **`**`** aceita `int ** int` e `real ** int`. O expoente é sempre `int`. Um expoente negativo em `int` produz NaT, e `0 ** 0 = 1`. Como em Python, `-2 ** 2 = -(2 ** 2) = -4`.
- **`real`** aceita `+ - * / **`, as comparações e o `-` unário. Os operadores `%`, `<<`, `>>`, `&&` e `||` sobre `real` são **erro de tipo**, assim como misturar `int` e `real` numa conta sem cast. Não há promoção automática como no C: escreva `(real) a / (real) b`.
- **`&&`** e **`||`** têm **curto-circuito**, como em Java: o lado direito só é avaliado se puder mudar o resultado. `- && x` é `−` e `+ || x` é `+` sem avaliar `x`, então `- && NaT = −`. Com `0` ou NaT à esquerda, o lado direito é avaliado (`NaT && x = NaT`). Os operandos são do tipo `trit`. Usar `tryte` ou `int` é **erro de tipo**; compare primeiro (`x > 0 && y > 0`).

### 3.1 Tabela de verdade K₃

| A | B | `A && B` | `A \|\| B` | `!A` |
|---|---|---|---|---|
| − | − | − | − | + |
| − | 0 | − | 0 | + |
| − | + | − | + | + |
| 0 | 0 | 0 | 0 | 0 |
| 0 | + | 0 | + | 0 |
| + | + | + | + | − |

---

## 4. Caracteres (`t6` simétrico)

| Valor | Caractere |
|---|---|
| 0 | espaço (memória expurgada aparece em branco) |
| +1 .. +26 | `A`–`Z` |
| −1 .. −26 | `a`–`z` (por isso `-'A' == 'a'`) |
| +27 .. +36 | `0`–`9` |
| −27 .. −53 | `\n` `\t` `.` `,` `:` `;` `!` `?` `-` `+` `/` `_` `=` `(` `)` `[` `]` `*` `<` `>` `@` `#` `%` `{` `}` `"` `'` |
| +54 .. +65 | `Á` `À` `Â` `Ã` `É` `Ê` `Í` `Ó` `Ô` `Õ` `Ú` `Ç` |
| −54 .. −65 | `á` `à` `â` `ã` `é` `ê` `í` `ó` `ô` `õ` `ú` `ç` (`-'Á' == 'á'`) |
| −66 .. −72 | `\|` `&` `\` `~` `^` `` ` `` `$` |
| demais | reservados. `emit` imprime `�` |

Atribuição exata da faixa de pontuação: −27 `\n`, −28 `\t`, −29 `.`, −30 `,`, −31 `:`, −32 `;`, −33 `!`, −34 `?`, −35 `-`, −36 `+`, −37 `/`, −38 `_`, −39 `=`, −40 `(`, −41 `)`, −42 `[`, −43 `]`, −44 `*`, −45 `<`, −46 `>`, −47 `@`, −48 `#`, −49 `%`, −50 `{`, −51 `}`, −52 `"`, −53 `'`; e −66 `|`, −67 `&`, −68 `\`, −69 `~`, −70 `^`, −71 `` ` ``, −72 `$`.

Dentro de textos e caracteres, `"` e `'` são escritos como `\"` e `\'`. Os escapes aceitos são `\n`, `\t`, `\"`, `\'` e `\\` (a própria barra).

---

## 5. Arrays

```
a[]: int = claim 4;                  // índices 0..3, células zeradas
a[]: int = {1, 2, 3, 4};             // o mesmo, já preenchido
sym s[]: int = claim 5;              // índices -2..+2
sym s[]: int = {-8, -1, 0, 1, 8};    // s[0] == 0, o valor do meio
n: int = len(a);                         // 4
```

Há dois tipos de array, e o tipo decide como os índices funcionam:

| Tipo | Índices | Tamanho | Uso típico |
|---|---|---|---|
| `x[]: T` | `0 .. N-1`, como em Java e C | Qualquer `N ≥ 0` | Listas, pilhas, texto em construção |
| `sym x[]: T` | `-k .. +k`, com o `0` no meio | Ímpar, `N = 2k+1` | Coisas centradas: deslocamentos, coordenadas, tabelas de 3ᵏ posições |

- `T` é qualquer tipo que não seja array: `trit`, `tryte`, `int`, `real`, `text`, um enum ou um struct. Os colchetes ficam no **nome**, como em `int x[]` do C. `sym` vem antes, como o `final`: `final sym x[]: int = ...`.
- `claim N` preenche com o zero do tipo: `0`, `0.0`, `""`, o número 0 num enum (que imprime `0` se não for um valor do enum) e, num struct, cada campo zerado.
- Num array de struct, `v[i]` é uma cópia do elemento, `v[i].x` lê um campo e `v[i].x = 5` muda só esse campo.
- `print(v)`, `{v}` e `toText(v)` mostram o array inteiro (`[1, 2, 3]`, `["Ana", "Bia"]`) só quando os elementos são números ou textos. Arrays de enum ou struct são impressos elemento por elemento.
- `{a, b, c}` cria um array já preenchido, como o inicializador do Java. Com `sym`, a quantidade de valores precisa ser ímpar.
- `claim N` com `N` negativo, `N` NaT ou (com `sym`) `N` par dispara **Trap**. Índice fora dos limites também.
- Quando o tamanho e o índice são conhecidos na compilação (array criado com `{...}` ou `claim 5`, índice literal), um índice fora dos limites é **erro de compilação**, como no Rust.
- `len(x)` devolve o tamanho.
- **Arrays mudam de tamanho** (só os que não são `sym`, que precisam ficar ímpares e centrados no 0). O array vem sempre primeiro:

| Comando | Efeito |
|---|---|
| `append(v, x)` | Põe `x` no fim |
| `pop(v)` | Tira o último elemento e o devolve. Vazio dispara Trap |
| `insert(v, i, x)` | Põe `x` na posição `i` (`0..len`), empurrando os seguintes |
| `remove(v, i)` | Tira o elemento da posição `i`, puxando os seguintes, e o devolve |
| `resize(v, n)` | Muda para `n` elementos. Ao crescer, completa com zeros. Ao diminuir, os elementos cortados precisam estar zerados, senão dispara Trap (proteção contra perder dados sem querer) |
| `clear(v)` | Deixa o array vazio |
| `replace(v, a..b, c..d)` | Troca os trechos `a..b` e `c..d` de lugar (inclusivos, sem sobreposição; os tamanhos podem ser diferentes). Funciona também em `sym` |

  Essas operações valem para arrays de qualquer tipo, fazendo o papel do `ArrayList` do Java (para uma lista com métodos, veja os genéricos, §8.12). Por dentro, a VM guarda uma folga, então `append` não copia o array a cada vez. Um array emprestado (`&v[]`) também pode mudar de tamanho. O compilador só confere índices literais de arrays que nunca mudam de tamanho no proc.
- Um array `sym` e um array a partir de 0 **não se misturam**: o mesmo índice aponta para células diferentes.
- Parâmetros dizem como recebem o array: `&v[]: int` (emprestado) ou `v[]: int` (recebe a posse, com `give`). No retorno, os colchetes vão no tipo, como em Java: `proc cria(n: int) -> int[]`.

## 6. Arrays e posse linear

| Tipo | Natureza | Pode |
|---|---|---|
| `x[]: T` (variável) / `x[]: T` (parâmetro) | Linear, dono único, não copiável | Ler, escrever, `give`, `burn`, `return` |
| `&x[]: T` (parâmetro ou variável) | Empréstimo | Ler e escrever |

Toda variável que guarda um array é dona dele. Não existe aritmética de ponteiros.

| Palavra | Equivale a (C/Rust) | Efeito |
|---|---|---|
| `claim N` | `malloc` / `new int[N]` | Aloca `N` células zeradas |
| `burn x` | `memset 0` + `free` | Zera o bloco, e o slot do dono vira NaT |
| `give x` | `move` | Transfere a posse, e o slot do dono vira NaT |
| `&x` | `&mut x` | Empresta durante uma chamada |

**Emprestar se escreve com `&`, como em Rust**, no tipo e na chamada: o parâmetro `c: &Conta` (um array: `&v[]: int`; um método: `&self`) recebe `&c`. Sem `&`, o parâmetro **recebe a posse** (`c: Conta`, `v[]: int`, `self`), e quem chama escreve `give c`.

### 6.1 Regras do empréstimo (`&`)

- Existe como parâmetro (`c: &Conta`) e como variável que só empresta (`atual: &No = &lista;`, veja §8.10). É criado com `&x` (ou `&a.campo`, `&v[i]`).
- Não pode ser retornado (`-> &T` é erro).
- Não aceita `burn` nem `give`.
- `f(&b, give b)` na mesma chamada é erro estático.
- `f(&b, &b)` também: o `proc` veria o mesmo array com dois nomes, e uma escrita por um mudaria o outro.

### 6.2 Regras da posse

Cada array ou objeto com dono deve ser consumido **exatamente uma vez** em todo caminho de execução, por `burn x`, `give x` ou `return x`.

Erros de compilação:

- Usar a variável depois de `give` ou `burn`.
- Chegar a `}` ou `return` com um dono vivo (vazamento).
- Copiar sem `give` (`sym c[]: int = b;`).
- Guardar `claim` ou `{...}` em uma variável que não seja array.
- Atribuir a um dono que ainda está vivo.
- Atribuir a um dono consumido por `give`. O nome só pode receber um novo `claim` **depois de `burn`**.
- Passar a um parâmetro que recebe a posse um argumento sem `give`.
- Usar como comando (`f();`) uma expressão que devolve um array ou objeto, o que o perderia.

### 6.3 Fluxo e linearidade

- **`tri`**: todos os ramos que não divergem devem terminar com o **mesmo** estado linear. Um ramo que diverge (`return`, `break`, `continue`) precisa ter consumido o que for exigido naquele ponto de saída.
- **`walk`/`while`/`for`**: o corpo não pode consumir um dono declarado fora do laço. Antes de `break` ou `continue`, todo dono declarado dentro do corpo deve estar consumido. A condição de um `while`/`for` roda a cada volta, então também não pode consumir nada (`while f(give v) > 0` é erro).
- **`&&` e `||`**: o lado direito pode não rodar (curto-circuito), então não pode ter `give`.
- **`on NaT`**: o bloco é obrigado a divergir (§8.5).

## 7. Classes de falha

| Classe | Causas | Efeito |
|---|---|---|
| **Conta que falha** | Overflow, divisão por zero, cast fora do intervalo, `toInt`/`toReal`/`input` inválido, conta com NaT | Trap com o motivo. Numa linha com `on NaT`, o valor vira NaT e o bloco trata |
| **Trap** (estrutura) | `claim` inválido, índice fora dos limites, `tri`/`if`/`while` sobre NaT, `if`/`while` sobre `0`, limite de `walk` NaT, ponteiro inválido, `burn` duplo | A VM **zera todo o heap e os registradores** e aborta com código ≠ 0 |

## 8. Sintaxe

A sintaxe segue o estilo C/Java/Rust. O que é próprio do Kestrel vem do ternário (`tri`, `<=>`, `walk` simétrico, `on NaT`) e da posse (`&`, `give`, `burn`).

**Convenção de nomes** (a mesma do Java): procs, variáveis e campos em `camelCase` (`readFile`, `idadeMinima`); tipos (`struct`, `enum`) em `PascalCase` (`TokenKind`); `const` em `MAIÚSCULAS` (`MAX_SIZE`). Os built-ins seguem essa convenção. O compilador não obriga, mas a biblioteca padrão e o `ksc.ks` seguem.

### 8.1 `proc` e `return`

```
proc sum(sym &v[]: int, k: int) -> int { ... return acc; }
proc main() -> int { ... return 0; }
```

O ponto de entrada é `main`. Um `proc` sem `-> T` usa `return;` sem valor.

### 8.2 Declaração e atribuição

```
x: int = 5;          // declaração: o tipo é sempre escrito
total: int?;         // sem valor: só com `?`, e começa null
x = x + 1;            // atribuição
x += 1;               // o mesmo que x = x + 1  (também -=  *=  /=  %=)
x++;                  // o mesmo que x += 1     (também x--)
```

- **Toda variável tem o tipo escrito.** O compilador nunca adivinha o tipo pelo valor (não existe o `var` do Java).
- **Declarar sem valor exige `?`**: `x: int;` é erro, `x: int?;` começa `null`. Arrays e structs nunca são null, então sempre recebem um valor (um array pode começar vazio: `{}`). `final` precisa de valor na declaração. Veja §8.2.1.
- `++` e `--` são **comandos**, não expressões (como em Go): `x = i++;` é erro, e não existe `++i`.
- Valem também em células de buffer: `b[i] += 2;`, `b[i]++;`.
- Em tri arms, escreva `- =>` com espaço: `-=>` seria lido como `-=` seguido de `>`.

Cada declaração ocupa um slot próprio, então um `x` declarado num bloco interno não sobrescreve o `x` de fora.

#### 8.2.1 `null` e tipos `T?` (null-safety, como em Kotlin)

```
idade: int? = toInt(texto);      // null se o texto não for um número
if idade != null {
    printl(idade + 1);           // aqui `idade` é int
}
n: int = toInt(texto);           // ERRO: pode ser null
k: int = toInt(texto) on NaT { return; };   // ou trate o null na linha

proc metade(x: int) -> int? {
    if x % 2 != 0 { return null; }
    return x / 2;
}

printl(n?.proximo?.valor);       // `?.`: null se algo no caminho for null
```

- **Um tipo sem `?` nunca é null.** `int`, `text`, `bool`, enums, objetos e interfaces ganham `?` para poder ser null: `int?`, `text?`, `Cor?`, `No?`. Arrays e structs não têm `?` (um struct pode ter campos `?`). Elementos de array também não.
- Na escrita é `null` (`x: int? = null;`, `return null;`). Por dentro, null é o NaT da VM. A palavra `NaT` só aparece no `on NaT`.
- **Conferir:** `x == null` e `x != null` devolvem `bool` e são o único teste que aceita null. Comparar um tipo sem `?` com null é erro (nunca seria verdade).
- **Usar um `T?` como `T`** (numa conta, numa condição, como argumento `T`, num método) só é permitido onde o compilador provou que não é null; senão é erro: `this is int?: it may be null`. Ele prova:
  - dentro de `if x != null { ... }` (e no `else` de `if x == null`);
  - depois de `if x == null { return; }` (ou `break`, `continue`, `exit`), até o fim do bloco;
  - no lado direito de `x != null && ...` e de `x == null || ...`;
  - dentro de `while x != null { ... }`;
  - depois de `assert(x != null);`;
  - depois de atribuir um valor que não é null (`x = 5;`).
- A prova vale para variáveis e caminhos de campos de objetos (`n.proximo.valor`), não para elementos de arrays nem campos de structs.
- **A prova se perde** quando algo pode ter mudado o valor: atribuir (`x = null`), atribuir dentro de um `if`/laço/`match` (depois dele, e no laço inteiro), `give` do campo, emprestar o dono numa chamada ou chamar um método do dono.
- **`a?.b`** e **`a?.m()`**: se `a` for null, o resultado é null (e o método não é chamado); senão, lê `b` ou chama `m`. O resultado é `T?`. Encadeia: `a?.b?.c`. Usar `?.` em algo que nunca é null é erro (use `.`).
- **Built-ins que podem não ter resultado devolvem `?`**: `toInt(s) -> int?`, `toReal(s) -> real?`, `input() -> int?`. Eles não disparam mais Trap: devolvem null.
- **Imprimir** um `T?` mostra `null` ou o valor (`printl`, `{x}`, `%s`). Um objeto `T?` precisa ser conferido antes de imprimir.
- **`on NaT`** também trata um null: `k: int = toInt(s) on NaT { return; };` dá a `k` o tipo `int`.

**`const` × `final`:**

| | `const` | `final` |
|---|---|---|
| Onde | Fora dos `proc` | Dentro dos `proc` |
| Valor | Fixo, conhecido na compilação (literais e contas com eles) | Qualquer expressão, até `input()` |
| Equivalente | `static final` do Java, `const` do Rust | `final` local do Java, `let` do Rust |

```
const MAX: int = 100;

proc main() {
    final idade: int = input();
    idade = 5;            // erro: `idade` is final and cannot change
    idade: int = 6;       // erro: não pode ser declarada de novo no mesmo bloco
}
```

- Num buffer `final`, as **células** continuam mudando (`b[0] = 5`), como num array `final` do Java. O que fica fixo é o nome. O `burn` continua obrigatório.
- Um bloco interno pode declarar outro `x`, que é uma variável nova.

### 8.3 `tri` (desvio triplo)

```
tri s <=> 10 {
    - => print("less");
    0 => print("equal");
    + => { print("greater"); print(s); }
}
```

- A expressão é do tipo `trit`. Os três ramos são obrigatórios e cada um aparece uma única vez.
- Um ramo é um comando terminado em `;` (como qualquer comando) ou um bloco `{ }`.
- Se a expressão for NaT, ocorre **Trap**.

### 8.3.1 `if` / `else`

```
if x < 0 {
    print("negativo");
} else if x == 0 {
    print("zero");
} else {
    print("positivo");
}
```

- A condição é um `trit`, como o `boolean` do Java. `if x` com `x: int` é erro de tipo.
- `+` entra no `if`, `−` vai para o `else`. **`0` (desconhecido) e NaT disparam Trap**: para tratar os três casos, use `tri`.
- Os parênteses são opcionais: `if (x < 0) { ... }` também vale. As chaves são obrigatórias.
- Por dentro, `if c { A } else { B }` é `tri c { - => B, 0 => trap, + => A }`. As regras de posse são as do `tri`: os dois ramos que continuam precisam deixar cada buffer no mesmo estado.

### 8.4 `walk` (laço simétrico)

```
walk i in -2..5 { ... }
walk i in span k { ... }   // o mesmo que -k..+k
```

- O intervalo é inclusivo e o passo é +1. Se `a > b`, o corpo não executa nenhuma vez.
- `a` e `b` são avaliados **uma única vez**, antes da primeira iteração. Se algum for NaT, ocorre **Trap**.
- `break` e `continue` são permitidos.
- `i` é `int` e só pode ser lido dentro do laço.

### 8.4.1 `while`

```
i: int = 0;
while i < 10 {
    print(i);
    i = i + 1;
}
while + { ... }            // laço infinito; saia com `break` ou `return`
```

- A condição é um `trit`, reavaliada antes de cada volta. `+` executa o corpo, `−` sai do laço. **`0` e NaT disparam Trap**, como no `if`.
- `break` sai do laço, e `continue` volta para a condição.
- Regra de posse igual à do `walk`: o corpo não pode consumir nem criar buffers que vivem fora do laço.

### 8.4.2 `for`

```
for i: int = 0; i < len(arr); i++ {     // começo; condição; passo
    print(arr[i]);
}
for i: int = -1; i <= 1; i++ { ... }     // array sym
for i: int = 10; i > 0; i -= 2 { ... }   // passo de -2
```

- É o `for` do Java sem os parênteses, como o do Go. As três partes são separadas por `;`.
- **Começo:** sempre declara o contador com o tipo escrito (`i: int = 0`). Reaproveitar uma variável de fora (`i = 0`) é erro.
- **Escopo:** o contador e tudo o que é declarado no corpo só existem dentro do laço, como em qualquer bloco `{ }`.
- **Condição:** um `trit`, testado antes de cada volta, com as regras do `while` (`0` e NaT disparam Trap).
- **Passo:** muda uma variável (`i++`, `i += 2`, `i = i * 3`). Roda depois do corpo e também no `continue`.
- `walk` continua existindo para intervalos simples (`walk i in 0..9`).

### 8.5 `on NaT` (barreira de veneno)

```
s: int = sum(&b, 2) on NaT { burn b; return -1; };
```

- É o `try/catch` do Kestrel, preso a uma linha. Enquanto a expressão é calculada, uma conta que falha produz NaT em vez de parar o programa.
- Se a expressão não for NaT, `s` recebe o valor.
- Caso contrário, executa o bloco, que precisa divergir (`return`, `break` ou `continue`).
- Só as contas da própria linha são tratadas. Se um proc chamado nela falhar, o programa para; para avisar quem chamou, o proc devolve `T?` e usa `return null;`.
- A expressão também pode ser um `T?` (`toInt(s)`, um proc `-> int?`): null vai para o bloco, e `s` fica com o tipo sem `?`.
- Depois do `;`, `s` com certeza não é null.

### 8.6 E/S

| Comando | Efeito |
|---|---|
| `print(x)` | Imprime um número (em decimal), um `text`, um enum, um struct ou um array, sem quebra de linha. Aceita `+` entre textos: `print("Olá, " + nome)`. Com vários valores, separa por espaço, como no Python: `print("total:", n)` → `total: 3` |
| `printl(x)` | Igual ao `print`, seguido de quebra de linha. `printl()` só quebra a linha |
| `print(b)` com array | Imprime os elementos: `[-8, -1, 0, 1, 8]`, `["Ana", "Bia"]`, como o `Arrays.toString` do Java (textos entre aspas). O buffer é emprestado só durante a impressão, então continua precisando de `burn` |
| `printf(fmt, args...)` | Imprime texto formatado (§8.6.1) |
| `len(s)` | Tamanho de um `text` |
| `toText(x)` | Converte um número em `text`, em decimal: `toText(-5)` → `"-5"`, `toText(1.5)` → `"1.5"`. Com um array, gera `"[-8, -1, 0, 1, 8]"` |
| `toText(x, 't')` / `toText(x, 'v')` | Converte um inteiro em `text` ternário (`"0t-++"`) ou heptavintimal (`"0ve"`), como o `Integer.toString(x, base)` do Java |
| `emit(c)` | Imprime um `tryte` como caractere (tabela §4) |
| `input()` | Lê um `int` (decimal, `0t` ou `0v`). Entrada inválida é uma conta que falha |
| `dump("label")` | Mostra os registradores e o heap |
| `readFile(caminho)` | O arquivo inteiro como `text`. Se não puder ler, **Trap** |
| `fileExists(caminho)` | `+` se o arquivo existe e pode ser lido |
| `writeFile(caminho, texto)` | Grava o texto (UTF-8), substituindo o arquivo |
| `writeBytes(caminho, arr)` | Grava cada célula de um array como um byte. Célula fora de `0..255` dispara **Trap** |
| `readLine()` | Uma linha da entrada padrão, sem o `\n`. No fim da entrada, devolve `""` |
| `eof()` | `+` se a entrada padrão acabou |
| `argCount()` / `arg(i)` | Argumentos do programa: `ksvm prog.ksb a b c` dá `arg(0) == "a"` |
| `exit(código)` | Encerra o programa com esse código de saída. É um comando que não volta, como `return`: um `proc` pode terminar nele |

#### 8.6.1 `printf`

O primeiro argumento é o **formato**: um texto literal, ou vários textos unidos com `+`. Os **literais** são lidos como formato. Uma **variável `text`** entra como está: um `%t` guardado nela é impresso literalmente. Somar um número ao formato é **erro de tipo**.

```
notacao: text = "%t";
printf("exemplo de " + notacao + " = %t\n", v);   // exemplo de %t = 0t+--
```

O formato aceita duas formas de colocar valores, que podem ser misturadas:

- **Chaves:** `{expr}` imprime o valor de qualquer expressão, como as f-strings do Python: `{nome}`, `{p.x}`, `{v[i].nome}`, `{media(a)}`, `{a + 1:t}`. O formato (`:t`, `:v`, `:c`) vem no fim. Dentro das chaves não pode haver outra `{` ou `}`.
- **Porcentagem:** cada `%` consome o próximo argumento, em ordem: `printf("%d + %d = %d", a, b, a + b)`.

| Forma | Imprime | Aceita |
|---|---|---|
| `{x}` | Valor normal (decimal, texto, nome do enum, struct ou array) | número, `text`, enum, struct, array |
| `%d` ou `%i` | Inteiro em decimal | `trit`, `tryte`, `int` |
| `%f` ou `%r` | Real | `real` |
| `%s` | Texto (ou o nome de um enum) | `text`, enum |
| `{x:t}` / `%t` | Ternário sem zeros à esquerda: `-5` → `0t-++` | `trit`, `tryte`, `int` |
| `{x:v}` / `%v` | Heptavintimal sem zeros à esquerda: `100` → `0vDh` | `trit`, `tryte`, `int` |
| `{x:c}` / `%c` | Caractere (tabela §4) | `trit`, `tryte` (um `int` precisa de cast: `(tryte) x`) |
| `{b}`, `{b:t}`, `{b:v}`, `%d`/`%t`/`%v` com buffer | Células entre colchetes: `[0t-0+ 0t- 0t0]` | array de `tryte` ou `int` |
| `{b:c}` / `%c` com buffer de `tryte` | As células como texto, sem colchetes: `Olá` (como o `println(char[])` do Java) | array de `tryte` |

- Tudo é verificado **na compilação**: tipo errado, `%` sem argumento, argumento sobrando ou nome desconhecido é **erro**.
- `{{`, `}}` e `%%` imprimem `{`, `}` e `%`. Fora do `printf`, esses caracteres são impressos normalmente.

### 8.7 `enum`

```
enum Cor { Vermelho, Verde, Azul }                 // 0, 1, 2
enum Http { Ok = 200, NotFound = 404, Teapot }     // 200, 404, 405

c: Cor = Cor.Verde;
print(c);            // Verde
print((int) c);      // 1
if c == Cor.Azul { ... }
```

- É declarado fora dos `proc`, como o `const`. O valor é escrito `Tipo.Nome`, como em Java.
- Os números começam em 0 e sobem de 1 em 1. Um valor explícito (`= 200`) reinicia a contagem a partir dele. Dois nomes com o mesmo número são erro.
- Um valor de enum é guardado como o seu número: não ocupa heap e não precisa de `burn`.
- Aceita só `==` e `!=` com outro valor do **mesmo** enum. Contas e `<`/`>` são erro de tipo. `(int) c` lê o número.
- `print(c)`, `{c}`, `%s` e `toText(c)` mostram o **nome**. Por dentro, cada enum ganha uma função escondida que converte número em nome.

### 8.8 `match`

```
match c {
    Cor.Vermelho => print("pare");
    Cor.Verde => print("siga");
    Cor.Azul => print("?");
}

match letra {
    'a' | 'e' | 'i' | 'o' | 'u' => print("vogal");
    '0' | '1' => { print("dígito"); }
    else => print("outro");
}
```

- Funciona com inteiros (`trit`, `tryte`, `int`) e enums. Os padrões são literais ou valores de enum; `|` junta vários num braço.
- Um braço é um comando terminado em `;` ou um bloco `{ }`, como no `tri` e no `switch` com seta do Java 14 (`case 2 -> x = 1;`). Só o primeiro braço que bater executa (não há *fall-through* como no `switch` do C).
- `else` pega o que sobrar e precisa ser o último braço.
- Um `match` de enum sem `else` precisa listar **todos** os valores; se faltar algum, é erro de compilação. Um `match` de inteiro sempre precisa de `else`.
- Repetir um valor em dois braços é erro. Um `match` sobre NaT dispara Trap.
- Por dentro, vira uma cadeia de `if`/`else`, então as regras de posse são as mesmas.

### 8.9 `struct`

```
struct Ponto {
    x: int;
    y: int;
}

struct Pessoa {
    nome: text;
    casa: Ponto;         // um struct dentro de outro
}

a: Ponto = Ponto { x: 1, y: 2 };
b: Ponto = a;              // cópia: mudar b não muda a
b.x = 10;
p: Pessoa = Pessoa { nome: "Ana", casa: Ponto { x: 3, y: 4 } };
p.casa.y++;
print(p);            // Pessoa { nome: "Ana", casa: Ponto { x: 3, y: 5 } }
```

- É declarado fora dos `proc`, como o `enum`. Cada campo termina com `;`, como uma variável.
- Os campos podem ser números, `text`, enums e outros structs. **Arrays não**: um struct é copiado como um número, e um array tem dono.
- Um struct não pode conter a si mesmo, nem por meio de outro struct.
- Para criar, **todos** os campos recebem valor (`Ponto { x: 1, y: 2 }`), em qualquer ordem. Faltar, repetir ou inventar um campo é erro. Os valores são calculados na ordem em que os campos foram declarados.
- **É copiado**, como um `int`: atribuir, passar para um proc e o `return` fazem cópias. Não precisa de `burn` e não usa `give`/`&`.
- `p.x` lê um campo e `p.x = v` (ou `+=`, `++`...) muda um campo. Com `final`, nem os campos mudam.
- `print`, `printl`, `{p}` e `%s` mostram o struct inteiro. `==`, `<` e as contas não existem para structs: compare os campos.
- Por dentro, um struct ocupa um slot por campo (um struct aninhado ocupa os slots dos seus campos). O `dump` mostra cada campo como `p.x`, `p.casa.y`.

### 8.10 `class`

```
class Conta {
    public nome: text;           // visível fora da classe
    saldo: real;                 // privado (o padrão)
    hist[]: real;                // um array que pertence ao objeto

    // Método sem `self`: "static". É o construtor: só dentro da classe se escreve `Conta { ... }`.
    public proc nova(nome: text) -> Conta {
        return Conta { nome: nome, saldo: 0.0, hist: {} };
    }
    public proc depositar(&self, v: real) {   // usa o objeto
        self.saldo += v;
        append(self.hist, v);
    }
    public proc fechar(self) -> real {        // consome o objeto
        s: real = self.saldo;
        burn self;
        return s;
    }
}

c: Conta = Conta.nova("Ana");
c.depositar(10.0);
printl(c);           // Conta { nome: "Ana", saldo: 10.0, hist: [10.0] }
total: real = c.fechar(); // depois disto, `c` não existe mais
```

- Uma classe junta dados e métodos. É declarada fora dos `proc`, e um arquivo pode ter várias.
- Campos e métodos são **privados** por padrão: só os métodos da própria classe os usam. `public` os deixa visíveis fora.
- O objeto vive no heap e **tem dono**, como um array: não é copiado (`d: Conta = c;` é erro), passa com `give`/`&` (parâmetros `c: Conta` / `c: &Conta`) e precisa de `burn`, `give` ou `return`.
- `burn c` libera também os arrays e objetos que os campos de `c` possuem.
- Um método recebe o objeto como primeiro parâmetro: `&self` (empresta: pode ler e mudar os campos) ou `self` (consome: precisa terminar com `burn self`, `give` ou `return`). `c.metodo(...)` passa `c` sozinho. Um método `self` só pode ser chamado numa variável que é dona do objeto.
- Método sem `self` é da classe ("static"): `Conta.nova(...)`. Não há construtor especial; o objeto é criado preenchendo todos os campos, e só dentro da classe.
- Campos podem ser de qualquer tipo, inclusive arrays e outros objetos. Um campo que possui algo (array ou objeto) não pode ser trocado, só alterado por dentro (`append(self.hist, x)`, `self.hist[i] = ...`). Lido de fora, ele vem emprestado.
- Uma classe não pode ter um campo **obrigatório** dela mesma (nem por outra classe), porque o primeiro objeto nunca poderia ser criado. Para árvores e listas ligadas, use um array (`filhos[]: No`, que pode começar vazio) ou um campo opcional (`proximo: No?`, veja abaixo). Um struct não pode conter objetos.
- Emprestar o mesmo objeto (ou um campo dele) duas vezes na mesma chamada é erro: `c.f(&c.hist)`.
- `print(c)`, `{c}` e `%s` mostram todos os campos, inclusive os privados (para depurar).

#### Arrays de objetos

```
contas[]: Conta = {give a, Conta.nova("Bia")};
append(contas, Conta.nova("Caio"));
contas[0].depositar(2.5);      // o elemento é emprestado para o método
ver(&contas[1]);           // empresta um elemento
x: Conta = remove(contas, 1);  // tira o objeto do array: agora `x` é o dono
burn x;
burn contas;                   // libera o array e todos os objetos dentro dele
```

- O array **é dono** dos seus objetos. Eles entram com `give`, ou já novos (`Conta.nova(...)`): `{a}` e `append(v, a)` sem `give` são erro, porque copiariam o objeto.
- `contas[i]` **fica no array**: chama métodos `&self`, lê campos públicos e passa com `&contas[i]`. Para ser dono dele, tire-o com `pop(v)` ou `remove(v, i)`. `x: Conta = contas[0];` é erro.
- Todo elemento é um objeto de verdade, nunca vazio. Por isso não existem para arrays de objetos: `claim n` (criaria elementos vazios), `resize` (crescer cria vazios, diminuir perderia objetos) e `clear` (perderia objetos). Trocar um elemento (`contas[0] = ...`) também é erro: tire o antigo com `remove`, dê `burn` nele, e ponha o novo com `insert`.
- `pop`/`remove` devolvem o objeto, que precisa ser guardado: `pop(contas);` sozinho é erro.
- Emprestar um elemento e tirar um elemento do mesmo array na mesma chamada é erro: `f(&contas[0], remove(contas, 0))`.
- `burn contas` libera cada objeto (e o que eles possuem) antes do array. Um campo `contas[]: Conta` dentro de outra classe funciona igual.
- O array inteiro não é impresso (como os de struct): imprima elemento por elemento.
#### Objetos opcionais (`T?`)

```
class No {
    public valor: int;
    public proximo: No?;          // pode ser null
    public proc novo(v: int) -> No { return No { valor: v, proximo: null }; }
}

proc soma(n: &No?) -> int {
    if n == null {
        return 0;
    }
    return n.valor + soma(&n.proximo);   // aqui `n` já foi conferido
}

if lista.proximo != null {
    printl(lista.proximo.valor);             // só aqui dentro
}
resto: No? = give lista.proximo;             // tira o objeto; o campo fica vazio
```

- Um objeto ou interface `T?` segue as regras de §8.2.1 (`null`, `x == null`, a prova antes de usar, `?.`). Vale em campos, variáveis, parâmetros (`n: No?`, `n: &No?`) e retornos (`-> No?`); não vale em arrays nem em structs.
- Usar o próprio campo (`n.proximo.metodo()`, `&n.proximo`) não anula a prova sobre ele; emprestar o dono ou chamar um método do dono anula.
- `give n.proximo` tira o objeto de um campo `T?` e o deixa vazio. Só campos `T?` podem ser dados assim.
- `n.proximo = x` e `v = x` (com `v: No?`) só funcionam se estiver **vazio**; senão dispara Trap (o objeto antigo seria perdido). Para trocar: `velho: No? = give n.proximo;`, `burn velho;` e então atribua.
- Uma variável `T?` tem dono como um objeto: precisa de `burn` (que não faz nada se estiver vazia) ou `give`. `burn` de um objeto libera também os campos `T?` que não estão vazios.
- Ao imprimir um objeto, um campo `T?` (ou de interface) aparece como `...`.

#### Variáveis emprestadas (`&`)

```
proc tamanho(lista: &No) -> int {
    n: int = 1;
    atual: &No? = &lista.proximo;   // só empresta: não é dono
    while atual != null {
        n++;
        atual = &atual.proximo;          // passa a emprestar outro
    }
    return n;
}

c: &Conta = &contas[0];
c.depositar(5.0);
burn contas;                                  // ok: `c` não é mais usado
```

- `x: &T = &caminho;` declara uma variável que **empresta** um array ou objeto (de uma variável, campo ou elemento), como um parâmetro `&`. Ela pode mudar para onde aponta (`x = &...;`, ou `x = null;` se for `T?`), mas não pode dar `burn` nem `give`.
- Serve para percorrer listas ligadas e árvores com laços, sem recursão (e sem o limite de 6561 chamadas).
- **Enquanto ela é usada** (até o último comando do bloco que a menciona), o dono do que ela empresta não pode: levar `burn` ou `give`, ter um elemento tirado (`pop`, `remove`, `give a.campo`), nem ser emprestado a uma chamada ou ter um método chamado. Usar a própria variável (`x.metodo()`, `&x`) pode, se ela for a única emprestando daquele dono.
- Ler o dono (campos, impressão) continua permitido.
- Uma variável `T?` com dono que é null ainda precisa de `burn` antes de um `return` (o `burn` de null não faz nada).

### 8.11 `interface`, `@impl` e `@override`

```
interface Forma {
    proc area(&self) -> real;
    proc nome(&self) -> text;
}

@impl(Forma)
class Circulo {
    raio: real;
    @override
    public proc area(&self) -> real { return 3.0 * self.raio * self.raio; }
    @override
    public proc nome(&self) -> text { return "circulo"; }
}

proc mostrar(f: &Forma) {          // aceita qualquer classe que implemente Forma
    printf("{f.nome()}: {f.area()}\n");
}

c: Circulo = Circulo.novo(1.0);
mostrar(&c);
```

- Uma `interface` lista métodos sem corpo, cada um terminado em `;`. Todos recebem `self` e são públicos.
- `@impl(A, B)` em cima da classe diz quais interfaces ela cumpre. O compilador confere que a classe tem **todos** os métodos de cada uma, públicos e com a mesma assinatura (`self`, parâmetros e retorno).
- **`@override` é obrigatório** em cada método que cumpre uma interface, e um `@override` que não cumpre nada é erro (pega erros de digitação). Não há herança: cada classe escreve os seus métodos.
- Um parâmetro `f: &Forma` aceita `&c` de qualquer classe que implemente `Forma`, ou `&f` de outro valor `Forma`. Por dentro, um valor de interface leva o ponteiro do objeto e o número da sua classe, e `f.area()` chama o `area` daquela classe.
- Pela interface só se chamam métodos dela: os campos e os outros métodos da classe não aparecem. Métodos `self` não podem ser chamados por uma interface (o objeto está emprestado).

#### Métodos padrão

```
interface Forma {
    proc area(&self) -> real;
    proc nome(&self) -> text;
    proc descrever(&self) -> text {       // padrão: vale para quem não escrever o seu
        return self.nome() + " de área " + toText(self.area());
    }
}
```

- Um método de interface com corpo é um **método padrão** (como no Java 8). Uma classe com `@impl(Forma)` que não o escreve ganha esse; uma que o escreve (com `@override`) usa o seu.
- Dentro dele, `self` é a interface: só os métodos dela aparecem, nunca os campos de uma classe. Precisa ser `&self`.
- Funciona pela interface (`f.descrever()`) e direto no objeto (`c.descrever()`).
- Não há herança (decidido até a 1.0): métodos padrão e composição ocupam o lugar dela.

#### Interfaces com dono

```
f: Forma = Circulo.novo(1.0);              // o objeto passa a ser visto como Forma
formas[]: Forma = {give f, Poligono.novo(3.0, 4.0)};
append(formas, Circulo.novo(2.0));
for i: int = 0; i < len(formas); i++ {
    mostrar(&formas[i]);               // cada uma chama o seu `area`
}
burn formas;                               // libera cada objeto, seja de que classe for

proc criar(quadrado: bool) -> Forma { ... } // pode devolver classes diferentes
```

- Uma variável, um campo, um parâmetro `f: Forma`, um retorno `-> Forma` ou um array `formas[]: Forma` pode **ser dono** de um objeto de qualquer classe que implemente a interface. O objeto entra já novo ou com `give`, e a conversão está escrita no tipo da declaração (`f: Forma = ...`), sem adivinhação. Uma classe que não implementa a interface é erro.
- O valor com dono segue as mesmas regras de um objeto: `give`, `&`, `return`, `burn`, e esquecer é erro. Declarado sem valor (`f: Forma;`), não pode ser usado antes de receber um.
- Pela interface só aparecem os métodos dela: `f.raio` é erro, e o valor não pode ser impresso inteiro (imprima o que os métodos devolvem).
- `burn f` olha a classe do objeto para liberar também o que os campos dele possuem.
- Arrays de interfaces seguem as regras dos arrays de objetos: `formas[i]` fica no array (emprestado), sai com `pop`/`remove`, e não valem `claim`, `resize`, `clear` nem `formas[i] = ...`.

#### Descobrir a classe: `is` e `match`

```
proc descrever(f: &Forma) {
    if f is Circulo {
        printl("raio", f.raio);       // aqui `f` é um Circulo
    } else {
        printl(f.nome());             // aqui continua sendo Forma
    }
}

match f {
    Circulo => printl(f.raio);
    Quadrado | Triangulo => printl(f.nome());
}

if f is Circulo {
    c: Circulo = give f;              // volta a ser um Circulo com dono
}
```

- `f is Circulo` devolve um `bool`: se o valor de interface guarda um objeto daquela classe (`false` se for um `T?` vazio). A classe precisa implementar a interface, senão é erro (nunca seria verdade). Como em Kotlin, usa-se `is` e não `==`: `==` compara valores, `is` pergunta o tipo.
- Onde o `is` provou a classe, **o próprio `f` passa a ser daquela classe**: campos públicos, todos os métodos, `&f` para um `c: &Circulo` e, se `f` tem dono, `give f` para um `Circulo`. Vale nos mesmos lugares da prova do `x != null` (§8.2.1): dentro do `if`, depois de `if !(f is C) { return; }`, depois de `&&`, e para variáveis e caminhos de campos (`d.principal`), não para elementos de arrays (`fs[i] is Circulo` funciona como teste, sem mudar o tipo).
- `match f { Classe => ...; }` escolhe pela classe. Num ramo com uma só classe, `f` é daquela classe; num ramo com várias (`A | B`), continua sendo a interface. Sem `else`, o `match` precisa cobrir todas as classes que implementam a interface. Funciona em variáveis e campos (para um elemento, passe-o a um proc).
- Não há um nome novo por ramo (`Circulo c =>`): seria um segundo nome para o mesmo objeto, e a posse evita isso. É como o `when` do Kotlin.

### 8.12 Genéricos

```
class List<T> {
    items[]: T;
    public proc new() -> List<T> {
        xs[]: T = {};
        return List<T> { items: give xs };
    }
    public proc add(&self, x: T) { append(self.items, give x); }
    public proc size(&self) -> int { return len(self.items); }
}

proc maior<T extends Forma>(a: &T, b: &T) -> real {
    if a.area() > b.area() { return a.area(); }
    return b.area();
}

ns: List<int> = List<int>.new();      // o tipo sempre escrito
ns.add(3);
qs: List<Quadrado> = List<Quadrado>.new();
m: real = maior<Quadrado>(&q, &p);
```

- Classes e procs podem ter **parâmetros de tipo**: `class Box<T>`, `class Map<K, V>`, `proc maior<T extends Forma>`. Structs, enums e métodos não (um método usa os parâmetros da classe).
- **O tipo é sempre escrito**, nunca deduzido: `List<int>.new()`, `maior<Quadrado>(...)`, `x: Box<text>`, `Box<T> { ... }` dentro da classe.
- `T` pode ser **qualquer tipo**: números (`List<int>`, sem as caixas `Integer` do Java), `text`, enums, structs, classes, interfaces, `T?` e outros genéricos (`List<List<int>>`). Arrays não (`List<int[]>`): use uma classe que guarde o array.
- **O código genérico é conferido uma vez para todo `T` possível.** Um `T` pode ser um objeto, então ele é tratado como um: move-se com `give`, empresta-se com `&`, precisa de `burn`, e não pode ser copiado (`return self.v` de um campo `T` é erro). Sem `extends`, um `T` não tem métodos nem campos.
- `T extends Forma`: o `T` precisa implementar a interface `Forma`, e o código pode chamar os métodos dela. Usar com um tipo que não a implementa é erro (`int` não implementa `Forma`).
- Quando `T` vira um valor (`List<int>`), `give`, `&` e `burn` dele apenas copiam (e o `burn` não faz nada).
- **Por dentro, há uma cópia do código por tipo usado** (como C++ e Rust), não *type erasure* como no Java: os valores têm tamanhos diferentes, e assim cada cópia é tão rápida quanto código escrito à mão.
- Ainda não: `Object` (`==`, `hashCode` e impressão em qualquer `T`), `Comparable` (`<` num `T`), coringas (`? extends T`, `? super T`), métodos devolvendo `&T`.

### 8.13 `assert`, `@test` e `@deprecated`

```
@test
proc somaFunciona() {
    assert(somar(2, 3) == 5);
    assert(somar(-1, 1) == 0, "soma com negativo");
}

@deprecated("use somar")
proc soma(a: int, b: int) -> int { return a + b; }
```

- `assert(cond);` confere que `cond` é verdade. Se for falsa (ou 0/NaT), dispara Trap com a linha e a condição como foi escrita: `assert failed (line 3): somar(2, 3) == 5`. Com uma mensagem, `assert(cond, "texto");`, ela aparece no lugar da condição. A mensagem é um texto literal. `assert` é um comando, não um valor.
- `@test` marca um proc **sem parâmetros e sem retorno** como teste. `ksc --test arquivo.ks` compila e roda cada teste numa VM separada, sem gravar o `.ksb`, e mostra `ok`/`FAILED` de cada um, o motivo e o que o teste imprimiu (só quando falha), e no fim `test result: N passed, M failed`. O comando sai com código 1 se algum teste falhou.
- Um teste falha se der Trap (um `assert` que falhou, uma divisão por zero...) ou se chamar `exit(n)` com `n` diferente de 0.
- Os testes são compilados junto com o programa (o compilador confere tipos e posse deles também), mas o `main` não os chama. Com `--test`, o arquivo pode nem ter `main`.
- `@deprecated` (ou `@deprecated("use outra")`) marca um proc, um método ou uma classe como **obsoleto**. Continua funcionando, mas cada uso fora dele mostra um aviso (`warning:`) com a mensagem, e a compilação segue. Dentro da própria classe (ou do próprio proc) não há aviso.
- Onde cada anotação vale: proc (`@test`, `@deprecated`), método (`@override`, `@deprecated`), classe (`@impl(...)`, `@deprecated`). Uma anotação desconhecida, ou no lugar errado, é erro.

### 8.14 Limites do compilador

| Limite | Valor | Ao passar |
|---|---|---|
| Aninhamento (blocos, parênteses, operadores em sequência) | 729 = 3⁶ níveis (um tryte) | Erro de compilação: "nested too deeply" |
| Variáveis por `proc` (slots: um struct ocupa um por campo) | 59.049 = 3¹⁰ | Erro de compilação |
| Tamanho de um struct (campos, contando os aninhados) | 59.049 = 3¹⁰ | Erro de compilação: "is too big" |
| Campos de um struct usado como elemento de array, ou de uma classe | 243 = 3⁵ | Erro de compilação |
| Parâmetros por `proc` (valores: um struct conta um por campo) | 243 = 3⁵ | Erro de compilação |
| Chamadas aninhadas na execução | 6.561 = 3⁸ | Trap: "call stack overflow" |

Todos os limites são potências de 3. Os de parâmetros e variáveis também cabem nos campos de 8 e 16 bits do `.ksb`.

## 9. BET-VM

- **Representação interna:** `i64`, com o NaT como sentinela `i64::MIN`. A forma `Word27` (BET em `u64`, 54 bits) só aparece no `.ksb`, no `dump` e na formatação `0t`/`0v`.
- **Pilha de avaliação** compartilhada, mais um **frame** por chamada.
- **Locais:** os índices 0..8 aparecem como os registradores simétricos `R-4 .. R+4`. Do índice 9 em diante, ficam na área de spill do frame, sem limite. Cada `let` recebe um slot próprio, então dois `x` em escopos diferentes não se sobrepõem.
- **Ponteiro:** `(índice: u32, geração: u32)`. O `burn` zera o bloco, incrementa a geração e devolve o bloco à free-list. Um acesso com geração antiga dispara **Trap**.

### 9.1 ISA

| Categoria | Opcodes |
|---|---|
| Pilha e locais | `PUSHC k` (int), `PUSHR k` (real), `LOAD s`, `STORE s`, `MOVE s`, `POP` |
| ALU (inteira) | `ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `POW`, `NEG`, `SHL3`, `SHR3`, `CMP3`, `KAND`, `KOR`, `TEST m` (resultado de `CMP3` → `trit`; os bits 0/1/2 de `m` dizem se −/0/+ vira `+`) |
| ALU (real) | `FADD`, `FSUB`, `FMUL`, `FDIV`, `FPOW`, `FNEG`, `FCMP3` |
| Texto | `PUSHS k`, `TLEN`, `TIDX`, `TSTR`, `TSTRT`, `TSTRV` (número → texto), `TCAT`, `TEQ`, `TSUB`, `TTOI`, `TTOF`, `CHARIS k` |
| Largura | `NARROW6`, `NARROW1`, `ITOF`, `FTOI` (overflow vira NaT) |
| Modo das falhas | `SOFT` (contas que falham produzem NaT; emitido antes do valor de uma linha com `on NaT`), `HARD` (volta ao padrão: Trap). Cada proc começa em `HARD` |
| Tamanho de arrays | `APPEND`, `POPEL`, `INSERT`, `REMOVE`, `RESIZE`, `CLEAR`, `REPLACE` (faixa `0x7_`; os valores do elemento vão abaixo do ponteiro na pilha) |
| Memória | `ALLOC` (sym), `ALLOCZ` (a partir de 0), `ALLOCT w sym` (elementos de `w` células, preenchidos com um modelo: `real`, `text`, struct), `PACK n sym` (literal `{...}`), `PACKT n w sym`, `LDIDX`, `STIDX`, `LDELEM off n` / `STELEM off n` (`n` células de um elemento, a partir da célula `off`: campos de struct), `PURGE s` |
| Fluxo | `JMP a`, `JNAT a`, `BR3 a- a0 a+`, `CALL f n` (`n` = valores passados; um struct conta um por campo), `RET`, `RETN n` (devolve os `n` valores de um struct), `TRAP str` |
| E/S | `INPUT`, `EMIT`, `PRINT`, `SHOW`, `SHOWT` (`0t`), `SHOWV` (`0v`), `DUMP str`, `READF`, `FEXISTS`, `WRITEF`, `WRITEB`, `READLN`, `EOF`, `ARGC`, `ARG`, `EXIT` |

**Codificação:** cada instrução tem 1 byte de opcode seguido de operandos de tamanho fixo, em little-endian. Os tipos são `k`, `f` e `a` (`u32`), `s` (`u16`) e `n` (`u8`). As faixas são: pilha `0x0_`, ALU inteira `0x1_`, ALU real `0x2_`, texto e conversão `0x3_`, memória `0x4_`, fluxo `0x5_` e E/S `0x6_`. A tabela completa está em `crates/kestrel-core/src/isa.rs`.

## 10. Formato `.ksb`

Todos os campos são little-endian.

```
Header (24 B)
  magic        [u8;4] = "K3B\x01"
  version      u16
  flags        u16
  entry_fn     u32          índice de main
  const_count  u32
  str_count    u32
  fn_count     u32
Const pool     const_count × u64     (Word27 BET)
String pool    str_count × (u32 len + bytes UTF-8)
Functions      fn_count ×
  name_len u16, name UTF-8
  param_count u8, local_count u16
  code_len u32 (em bytes)
  code: opcode u8 + operandos fixos
  [se flags & 1] local_count × (u16 len + nome UTF-8)
```

- **Bit 0 de `flags`** indica que o arquivo traz os nomes das variáveis, para o `dump` mostrar `R0 (acc)`.
- **Verificação na leitura:** o arquivo é rejeitado se algum salto cair fora do início de uma instrução, se um índice de constante, string, função ou slot não existir, ou se o número de argumentos de uma chamada não bater com os parâmetros da função chamada.

Os saltos usam um **offset absoluto em bytes** (`u32`) dentro do código da própria função.

---

## 11. Gramática (EBNF)

```ebnf
program    = { const_decl | proc_decl | enum_decl | struct_decl | class_decl | iface_decl } ;
iface_decl = "interface" IDENT "{" { "proc" IDENT "(" [ "&" ] "self" { "," param } ")" [ "->" ret_type ] ";" } "}" ;
class_decl = { annotation } "class" IDENT "{" { member } "}" ;
member     = [ "public" | "private" ] ( field | proc_decl ) ;
field      = [ "sym" ] IDENT [ "[" "]" ] ":" type ";" ;
annotation = "@" IDENT [ "(" IDENT { "," IDENT } ")" ] ;
struct_decl = "struct" IDENT "{" { IDENT ":" type ";" } "}" ;
enum_decl  = "enum" IDENT "{" [ IDENT [ "=" expr ] { "," IDENT [ "=" expr ] } [ "," ] ] "}" ;
const_decl = "const" IDENT ":" scalar "=" expr ";" ;
proc_decl  = "proc" IDENT "(" [ param { "," param } ] ")" [ "->" ret_type ] block ;
param      = IDENT ":" [ "&" ] type | [ "sym" ] [ "&" ] IDENT "[" "]" ":" elem | [ "&" ] "self" ;
ret_type   = type | [ "sym" ] elem "[" "]" ;
type       = scalar | "bool" | "text" | IDENT ;
scalar     = "trit" | "tryte" | "int" | "real" ;
elem       = "tryte" | "int" ;

block      = "{" { stmt } "}" ;
stmt       = [ "final" ] IDENT ":" type "=" expr [ on_nat ] ";"
           | [ "final" ] [ "sym" ] IDENT "[" "]" ":" elem "=" expr ";"
           | IDENT ":" type ";"
           | [ "sym" ] IDENT "[" "]" ":" elem ";"
           | place ( "=" | "+=" | "-=" | "*=" | "/=" | "%=" ) expr ";"
           | place ( "++" | "--" ) ";"
           | "burn" IDENT ";"
           | "return" [ expr ] ";"
           | "break" ";" | "continue" ";"
           | "tri" tri_head "{" arm arm arm "}"
           | "if" expr block [ "else" ( block | if_stmt ) ]
           | "walk" IDENT "in" range block
           | "while" expr block
           | "for" IDENT ":" type "=" expr ";" expr ";" assign block
           | "match" expr "{" { match_arm } "}"
           | io ";"
           | expr ";" ;
on_nat     = "on" "NaT" block ;
place      = IDENT [ "[" expr "]" ] { "." IDENT } ;
tri_head   = expr ;
arm        = ( "-" | "0" | "+" ) "=>" ( block | stmt ) ;
range      = expr ".." expr | "span" expr ;
io         = "print" "(" expr ")" | "printl" "(" [ expr ] ")" | "emit" "(" expr ")"
           | "printf" "(" STRING { "," expr } ")" | "dump" "(" STRING ")" ;

expr       = or_expr ;
or_expr    = and_expr { "||" and_expr } ;
and_expr   = cmp_expr { "&&" cmp_expr } ;
cmp_expr   = add_expr [ ( "<=>" | "==" | "!=" | "<" | ">" | "<=" | ">=" ) add_expr ] ;
add_expr   = mul_expr { ( "+" | "-" ) mul_expr } ;
mul_expr   = sh_expr  { ( "*" | "/" | "%" ) sh_expr } ;
sh_expr    = unary { ( "<<" | ">>" ) unary } ;
unary      = ( "!" | "-" | "(" ( scalar | "bool" ) ")" ) unary | power ;
power      = postfix [ "**" unary ] ;
postfix    = primary { "[" expr "]" | "." IDENT } ;
match_arm  = ( expr { "|" expr } | "else" ) "=>" ( block | stmt ) ;
primary    = literal | IDENT | IDENT "(" [ arg { "," arg } ] ")"
           | "claim" expr | "{" [ expr { "," expr } [ "," ] ] "}"
           | IDENT "{" IDENT ":" expr { "," IDENT ":" expr } [ "," ] "}"
           | "input" "(" ")" | "len" "(" expr ")" | "(" expr ")" ;
arg        = [ "give" | "&" ] expr ;
```


### Exemplo

```
proc sum(sym &v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    return acc;
}

proc main() -> int {
    sym b[]: int = claim 5;       // índices -2..+2
    walk i in span 2 {
        b[i] = i * i;
    }

    s: int = sum(&b, 2) on NaT { burn b; return -1; };
    s = s << 1;                    // s = s * 3

    tri s <=> 10 {
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;
    return 0;
}
```

## 12. Pendências

Nenhuma. Especificação v0.1 fechada.
