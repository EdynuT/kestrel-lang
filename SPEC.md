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
| `trit` | 1 trit | −1 .. +1 |
| `tryte` | 6 trits (t6) | −364 .. +364 |
| `int` | 27 trits (t27) | −3.812.798.742.493 .. +3.812.798.742.493 |
| `real` | 27 trits (t27) | ≈ ±10¹⁷⁴, com cerca de 10 dígitos decimais de precisão (§1.5) |
| `text` | referência | String literal imutável (§1.6) |
| enum declarado (`Cor`) | 27 trits (t27) | Um dos valores listados na declaração (§8.7) |

Neste documento, `t6` e `t27` indicam larguras. No código-fonte, os tipos se escrevem `tryte` e `int`.

### 1.3 NaT (veneno)

- O NaT vale para a **palavra inteira**. Basta um trit `?` para a palavra toda ser NaT.
- Qualquer operação com um operando NaT produz NaT.
- Na fonte, o NaT se escreve com a palavra-chave `NaT` ou com qualquer literal que contenha `?`.

### 1.4 Conversões

- A **ampliação** `trit → t6 → t27` é implícita e não perde informação.
- O **estreitamento** exige um *cast* no estilo C: `(tryte) x`, `(trit) x`. Um valor fora do intervalo vira **NaT**.
- A conversão entre `int` e `real` é **sempre explícita**. `(real) x` arredonda para o valor mais próximo, e `(int) r` trunca em direção a zero (fora do intervalo vira NaT).

### 1.5 `real` (ponto flutuante ternário)

```
int de 27 trits:  [ 6 trits expoente | 21 trits mantissa ]
valor = (mantissa / 3²⁰) × 3^expoente
```

- A mantissa é **normalizada**: o trit mais significativo é sempre `-` ou `+`, exceto no zero. Não há trit de sinal, porque o sinal está na própria mantissa.
- O expoente é um `tryte`, de −364 a +364.
- O arredondamento é sempre para o valor mais próximo, que é o efeito natural de descartar trits.
- Overflow e divisão por zero produzem **NaT**. **Não existe infinito.** Um underflow vira 0.
- Frações de 3 são exatas: `1.0 / 3.0` não tem erro de arredondamento.

### 1.6 `text` (string)

- É um literal imutável guardado no pool de strings do `.ksb`, como o `const char*` do C. É copiável e **não precisa de `burn`**.
- `s[i]` devolve o `tryte` da posição `i`. **O índice começa em 0**, como em C (como os arrays sem `sym`). Índice fora de `0 .. len(s)-1` dispara **Trap**.
- `len(s)` devolve o tamanho como `int`.
- `print(s)` imprime o texto. Dentro do `print`, `+` junta textos: `print("Olá, " + nome + "\n")`. Só vale **texto + texto**: `"x = " + 5` é **erro de tipo**. Fora do `print`, `+` com texto também é erro (o texto não pode ser criado em tempo de execução).
- Cada caractere precisa existir na tabela §4. Um caractere fora dela, como `&` ou `ñ`, é **erro de compilação**.
- `to_text(x)` cria um texto novo a partir de um número (§8.6). Os textos criados assim duram até o fim do programa (limite de 2²⁴ caracteres no total). Para montar texto caractere por caractere, use um array de `tryte`, como um array de `char` em C.

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

Regras:

- **Overflow** além da largura do tipo resultante produz **NaT**. Não existe wrap-around.
- **`/`** trunca em direção a zero. Divisão por zero produz NaT.
- **`%`** é o resto da divisão. O sinal segue o dividendo, e vale `a = (a/b)·b + a%b`. Resto por zero produz NaT.
- **`>>`** descarta os trits inferiores, o que equivale a arredondar para o inteiro mais próximo. Exemplo: `2 >> 1 = 1`, enquanto `2 / 3 = 0`.
- **`<<`** produz NaT se algum trit não nulo passar da largura do tipo. Com `k ≥ 27`, o resultado é `0` se o operando for 0 e NaT caso contrário. Com `k < 0` ou `k` NaT, o resultado é NaT.
- **`!x`** inverte um `trit` (`!+ = −`, `!0 = 0`). Sobre números é erro de tipo; use `-x`. Os dois compilam para o mesmo opcode `NEG`, porque em ternário balanceado negar é inverter cada trit.
- As **comparações** aceitam números do mesmo tipo (`int` com `real` exige cast). Se um lado for NaT, o resultado é NaT. Comparar textos chega na 0.4.
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
| demais | reservados. `emit` imprime `�` |

Atribuição exata da faixa de pontuação: −27 `\n`, −28 `\t`, −29 `.`, −30 `,`, −31 `:`, −32 `;`, −33 `!`, −34 `?`, −35 `-`, −36 `+`, −37 `/`, −38 `_`, −39 `=`, −40 `(`, −41 `)`, −42 `[`, −43 `]`, −44 `*`, −45 `<`, −46 `>`, −47 `@`, −48 `#`, −49 `%`, −50 `{`, −51 `}`, −52 `"`, −53 `'`.

Dentro de textos e caracteres, `"` e `'` são escritos como `\"` e `\'`. Os escapes aceitos são `\n`, `\t`, `\"` e `\'`.

---

## 5. Arrays

```
a[]: int = claim 4;                  // índices 0..3, células zeradas
a[]: int = {1, 2, 3, 4};             // o mesmo, já preenchido
sym s[]: int = claim 5;              // índices -2..+2
sym s[]: int = {-8, -1, 0, 1, 8};    // s[0] == 0, o valor do meio
n := len(a);                         // 4
```

Há dois tipos de array, e o tipo decide como os índices funcionam:

| Tipo | Índices | Tamanho | Uso típico |
|---|---|---|---|
| `x[]: T` | `0 .. N-1`, como em Java e C | Qualquer `N ≥ 0` | Listas, pilhas, texto em construção |
| `sym x[]: T` | `-k .. +k`, com o `0` no meio | Ímpar, `N = 2k+1` | Coisas centradas: deslocamentos, coordenadas, tabelas de 3ᵏ posições |

- `T` é `tryte` ou `int`. Os colchetes ficam no **nome**, como em `int x[]` do C. `sym` vem antes, como o `final`: `final sym x[]: int = ...`.
- `{a, b, c}` cria um array já preenchido, como o inicializador do Java. Com `sym`, a quantidade de valores precisa ser ímpar.
- `claim N` com `N` negativo, `N` NaT ou (com `sym`) `N` par dispara **Trap**. Índice fora dos limites também.
- Quando o tamanho e o índice são conhecidos na compilação (array criado com `{...}` ou `claim 5`, índice literal), um índice fora dos limites é **erro de compilação**, como no Rust.
- `len(x)` devolve o tamanho.
- Um array `sym` e um array a partir de 0 **não se misturam**: o mesmo índice aponta para células diferentes.
- Parâmetros dizem como recebem o array: `lent v[]: int` (emprestado) ou `own v[]: int` (recebe a posse, com `give`). No retorno, os colchetes vão no tipo, como em Java: `proc cria(n: int) -> int[]`.

## 6. Arrays e posse linear

| Tipo | Natureza | Pode |
|---|---|---|
| `x[]: T` (variável) / `own x[]: T` (parâmetro) | Linear, dono único, não copiável | Ler, escrever, `give`, `burn`, `yield` |
| `lent x[]: T` (só parâmetro) | Empréstimo, **segunda classe** | Ler e escrever |

Toda variável que guarda um array é dona dele. Não existe aritmética de ponteiros.

| Palavra | Equivale a (C/Rust) | Efeito |
|---|---|---|
| `claim N` | `malloc` / `new int[N]` | Aloca `N` células zeradas |
| `burn x` | `memset 0` + `free` | Zera o bloco, e o slot do dono vira NaT |
| `give x` | `move` | Transfere a posse, e o slot do dono vira NaT |
| `lend x` | `&mut x` | Empresta durante uma chamada |

### 6.1 Regras do `lent`

- Existe **somente como parâmetro** de `proc`, e é criado com `lend x` no argumento.
- Não pode ser retornado nem guardado em variável local.
- Não aceita `burn` nem `give`.
- `f(lend b, give b)` na mesma chamada é erro estático.
- `f(lend b, lend b)` também: o `proc` veria o mesmo array com dois nomes, e uma escrita por um mudaria o outro.

### 6.2 Regras do `own`

Cada `own` deve ser consumido **exatamente uma vez** em todo caminho de execução, por `burn x`, `give x` ou `yield x`.

Erros de compilação:

- Usar a variável depois de `give` ou `burn`.
- Chegar a `}` ou `yield` com um `own` vivo (vazamento).
- Copiar sem `give` (`sym c[]: int = b;`).
- Guardar `claim` ou `{...}` em uma variável que não seja array.
- Atribuir a um `own` que ainda está vivo.
- Atribuir a um `own` consumido por `give`. O nome só pode receber um novo `claim` **depois de `burn`**.
- Passar a um parâmetro `own` um argumento sem `give`.
- Usar como comando (`f();`) uma expressão de tipo `own`, o que descartaria o ponteiro.

### 6.3 Fluxo e linearidade

- **`tri`**: todos os ramos que não divergem devem terminar com o **mesmo** estado linear. Um ramo que diverge (`yield`, `break`, `continue`) precisa ter consumido o que for exigido naquele ponto de saída.
- **`walk`**: o corpo não pode consumir um `own` declarado fora do laço. Antes de `break` ou `continue`, todo `own` declarado dentro do corpo deve estar consumido.
- **`on NaT`**: o bloco é obrigado a divergir (§8.5).

## 7. Classes de falha

| Classe | Causas | Efeito |
|---|---|---|
| **NaT** (dado) | Overflow, divisão por zero, cast fora do intervalo, `input` inválido | O valor vira NaT e pode ser interceptado por `on NaT` |
| **Trap** (estrutura) | `claim` inválido, índice fora dos limites, `tri`/`if`/`while` sobre NaT, `if`/`while` sobre `0`, limite de `walk` NaT, ponteiro inválido, `burn` duplo | A VM **zera todo o heap e os registradores** e aborta com código ≠ 0 |

## 8. Sintaxe

A sintaxe segue o estilo C/Java/Rust. O que é próprio do Kestrel vem do ternário (`tri`, `<=>`, `walk` simétrico, `on NaT`) e da posse (`own`, `lend`, `give`, `burn`).

### 8.1 `proc` e `yield`

```
proc sum(lent sym v[]: int, k: int) -> int { ... yield acc; }
proc main() -> int { ... yield 0; }
```

O ponto de entrada é `main`. Um `proc` sem `-> T` usa `yield;` sem valor.

### 8.2 Declaração e atribuição

```
x: int = 5;          // tipo explícito
y := x * 3;           // tipo inferido do valor
x = x + 1;            // atribuição
x += 1;               // o mesmo que x = x + 1  (também -=  *=  /=  %=)
x++;                  // o mesmo que x += 1     (também x--)
```

- `++` e `--` são **comandos**, não expressões (como em Go): `x = i++;` é erro, e não existe `++i`.
- Valem também em células de buffer: `b[i] += 2;`, `b[i]++;`.
- Em tri arms, escreva `- =>` com espaço: `-=>` seria lido como `-=` seguido de `>`.

Cada declaração ocupa um slot próprio, então um `x` declarado num bloco interno não sobrescreve o `x` de fora.

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
    idade := 6;           // erro: não pode ser declarada de novo no mesmo bloco
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
i := 0;
while i < 10 {
    print(i);
    i = i + 1;
}
while + { ... }            // laço infinito; saia com `break` ou `yield`
```

- A condição é um `trit`, reavaliada antes de cada volta. `+` executa o corpo, `−` sai do laço. **`0` e NaT disparam Trap**, como no `if`.
- `break` sai do laço, e `continue` volta para a condição.
- Regra de posse igual à do `walk`: o corpo não pode consumir nem criar buffers que vivem fora do laço.

### 8.4.2 `for`

```
for i := 0; i < len(arr); i++ {     // começo; condição; passo
    print(arr[i]);
}
for i := -1; i <= 1; i++ { ... }     // array sym
for i := 10; i > 0; i -= 2 { ... }   // passo de -2
```

- É o `for` do Java sem os parênteses, como o do Go. As três partes são separadas por `;`.
- **Começo:** declara (`i := 0`) ou atribui (`i = 0`) uma variável. Uma variável declarada aqui só existe dentro do laço.
- **Condição:** um `trit`, testado antes de cada volta, com as regras do `while` (`0` e NaT disparam Trap).
- **Passo:** muda uma variável (`i++`, `i += 2`, `i = i * 3`). Roda depois do corpo e também no `continue`.
- `walk` continua existindo para intervalos simples (`walk i in 0..9`).

### 8.5 `on NaT` (barreira de veneno)

```
s: int = sum(lend b, 2) on NaT { burn b; yield -1; };
```

- Se a expressão não for NaT, `s` recebe o valor.
- Caso contrário, executa o bloco, que precisa divergir (`yield`, `break` ou `continue`).
- Depois do `;`, `s` com certeza não é NaT.

### 8.6 E/S

| Comando | Efeito |
|---|---|
| `print(x)` | Imprime um número (em decimal) ou um `text`, sem quebra de linha. Aceita `+` entre textos: `print("Olá, " + nome)` |
| `printl(x)` | Igual ao `print`, seguido de quebra de linha. `printl()` só quebra a linha |
| `print(b)` com buffer | Imprime as células: `[-8 -1 0 1 8]`, como o `Arrays.toString` do Java. O buffer é emprestado só durante a impressão, então continua precisando de `burn` |
| `printf(fmt, args...)` | Imprime texto formatado (§8.6.1) |
| `len(s)` | Tamanho de um `text` |
| `to_text(x)` | Converte um número em `text`, em decimal: `to_text(-5)` → `"-5"`, `to_text(1.5)` → `"1.5"`. Com um buffer, gera `"[-8 -1 0 1 8]"` |
| `to_text(x, 't')` / `to_text(x, 'v')` | Converte um inteiro em `text` ternário (`"0t-++"`) ou heptavintimal (`"0ve"`), como o `Integer.toString(x, base)` do Java |
| `emit(c)` | Imprime um `tryte` como caractere (tabela §4) |
| `input()` | Lê um `int` (decimal, `0t` ou `0v`). Entrada inválida vira NaT |
| `dump("label")` | Mostra os registradores e o heap |

#### 8.6.1 `printf`

O primeiro argumento é o **formato**: um texto literal, ou vários textos unidos com `+`. Os **literais** são lidos como formato. Uma **variável `text`** entra como está: um `%t` guardado nela é impresso literalmente. Somar um número ao formato é **erro de tipo**.

```
notacao: text = "%t";
printf("exemplo de " + notacao + " = %t\n", v);   // exemplo de %t = 0t+--
```

O formato aceita duas formas de colocar valores, que podem ser misturadas:

- **Chaves:** `{nome}` imprime a variável `nome`. Só aceita nomes de variáveis e constantes. Contas vão como argumento `%`.
- **Porcentagem:** cada `%` consome o próximo argumento, em ordem: `printf("%d + %d = %d", a, b, a + b)`.

| Forma | Imprime | Aceita |
|---|---|---|
| `{x}` | Valor normal (decimal, texto ou nome do enum) | número, `text` ou enum |
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
- `print(c)`, `{c}`, `%s` e `to_text(c)` mostram o **nome**. Por dentro, cada enum ganha uma função escondida que converte número em nome.

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

### 8.9 Limites do compilador

| Limite | Valor | Ao passar |
|---|---|---|
| Aninhamento (blocos, parênteses, operadores em sequência) | 729 = 3⁶ níveis (um tryte) | Erro de compilação: "nested too deeply" |
| Variáveis por `proc` | 59.049 = 3¹⁰ | Erro de compilação |
| Parâmetros por `proc` | 243 = 3⁵ | Erro de compilação |
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
| Texto | `PUSHS k`, `TLEN`, `TIDX`, `TSTR`, `TSTRT`, `TSTRV` (número → texto) |
| Largura | `NARROW6`, `NARROW1`, `ITOF`, `FTOI` (overflow vira NaT) |
| Memória | `ALLOC` (sym), `ALLOCZ` (a partir de 0), `PACK n sym` (literal `{...}`), `LDIDX`, `STIDX`, `PURGE s` |
| Fluxo | `JMP a`, `JNAT a`, `BR3 a- a0 a+`, `CALL f n`, `RET`, `TRAP str` |
| E/S | `INPUT`, `EMIT`, `PRINT`, `SHOW`, `SHOWT` (`0t`), `SHOWV` (`0v`), `DUMP str` |

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
program    = { const_decl | proc_decl | enum_decl } ;
enum_decl  = "enum" IDENT "{" [ IDENT [ "=" expr ] { "," IDENT [ "=" expr ] } [ "," ] ] "}" ;
const_decl = "const" IDENT ":" scalar "=" expr ";" ;
proc_decl  = "proc" IDENT "(" [ param { "," param } ] ")" [ "->" ret_type ] block ;
param      = IDENT ":" type | ( "own" | "lent" ) [ "sym" ] IDENT "[" "]" ":" elem ;
ret_type   = type | [ "sym" ] elem "[" "]" ;
type       = scalar | "text" | IDENT ;
scalar     = "trit" | "tryte" | "int" | "real" ;
elem       = "tryte" | "int" ;

block      = "{" { stmt } "}" ;
stmt       = [ "final" ] IDENT ":" type "=" expr [ on_nat ] ";"
           | [ "final" ] [ "sym" ] IDENT "[" "]" ":" elem "=" expr ";"
           | [ "final" ] IDENT ":=" expr [ on_nat ] ";"
           | place ( "=" | "+=" | "-=" | "*=" | "/=" | "%=" ) expr ";"
           | place ( "++" | "--" ) ";"
           | "burn" IDENT ";"
           | "yield" [ expr ] ";"
           | "break" ";" | "continue" ";"
           | "tri" tri_head "{" arm arm arm "}"
           | "if" expr block [ "else" ( block | if_stmt ) ]
           | "walk" IDENT "in" range block
           | "while" expr block
           | "for" ( decl | assign ) ";" expr ";" assign block
           | "match" expr "{" { match_arm } "}"
           | io ";"
           | expr ";" ;
on_nat     = "on" "NaT" block ;
place      = IDENT [ "[" expr "]" ] ;
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
unary      = ( "!" | "-" | "(" scalar ")" ) unary | power ;
power      = postfix [ "**" unary ] ;
postfix    = primary { "[" expr "]" | "." IDENT } ;
match_arm  = ( expr { "|" expr } | "else" ) "=>" ( block | stmt ) ;
primary    = literal | IDENT | IDENT "(" [ arg { "," arg } ] ")"
           | "claim" expr | "{" [ expr { "," expr } [ "," ] ] "}"
           | "input" "(" ")" | "len" "(" expr ")" | "(" expr ")" ;
arg        = [ "give" | "lend" ] expr ;
```


### Exemplo

```
proc sum(lent sym v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    yield acc;
}

proc main() -> int {
    sym b[]: int = claim 5;       // índices -2..+2
    walk i in span 2 {
        b[i] = i * i;
    }

    s: int = sum(lend b, 2) on NaT { burn b; yield -1; };
    s = s << 1;                    // s = s * 3

    tri s <=> 10 {
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;
    yield 0;
}
```

## 12. Pendências

Nenhuma. Especificação v0.1 fechada.
