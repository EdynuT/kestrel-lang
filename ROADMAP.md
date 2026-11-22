# Kestrel — Roteiro até a 1.0

**Meta da 1.0:** o compilador do Kestrel (`ksc`) escrito **em Kestrel** compila a si mesmo (*bootstrap*).

A máquina virtual (`ksvm`) continua em Rust, porque não existe hardware ternário para rodar o código direto. É a mesma divisão do Java: o `javac` é escrito em Java, e a JVM em C++.

```
hoje:  programa.ks ──[ksc em Rust]────────────────────► programa.ksb ──[ksvm]──► roda
1.0:   programa.ks ──[ksc.ksb (Kestrel) rodando na ksvm]──► programa.ksb ──[ksvm]──► roda
```

## Versões

| Versão | Conteúdo | Estado |
|---|---|---|
| 0.1 | Compilador (`ksc`) e VM (`ksvm`) em Rust, de ponta a ponta | ✅ |
| 0.2 | `int`, cast `(T) x`, `print`/`printl`/`printf`, `toText`, `final`, impressão de buffers, tabela de caracteres ampliada | ✅ |
| 0.3 | **Controle e arrays:** `if`/`else`, `while`, `for`, comparações (`== != < > <= >=`), `+=`/`++`, enums, `match`, arrays a partir de 0 (`x[]: int`) e simétricos (`sym x[]: int`), literal `{1, 2, 3}` | ✅ |
| 0.4 | **Textos, E/S e erros:** comparar, cortar, montar e converter textos, classificar caracteres, ler arquivo e stdin, escrever bytes, argumentos de linha de comando, `exit`, contas que falham disparam Trap e `on NaT` trata | ✅ |
| 0.5 | **Dados:** `struct` (só dados), arrays de qualquer tipo (inclusive structs), arrays que mudam de tamanho (`append`, `pop`, `insert`, `remove`, `resize`, `clear`, `replace`) | ✅ |
| 0.6 | **Classes:** `class` com métodos, `private`/`public`, interfaces (sem herança), arrays de objetos, interfaces com dono, árvores e objetos opcionais (`T?`), anotações (`@impl`, `@override`, `@test`, `@deprecated`), `assert` e `ksc --test` | ✅ |
| 0.7 | **Ferramentas:** módulos (`use`), seção de símbolos e tabela de linhas no `.ksb`, documentação `/* */`, executável único, traceback com linha do `.ks`, biblioteca padrão mínima escrita em Kestrel (hash map, hash set, funções de texto) | |
| 0.8 | `ksc.ks`: lexer e parser | |
| 0.9 | `ksc.ks`: verificação de tipos e de posse, gerador de código e escrita do `.ksb` | |
| **1.0** | Bootstrap: o teste abaixo passa | |

A biblioteca padrão da 0.7 e o `ksc.ks` (0.8 e 0.9) são escritos como **aulas**: o código Kestrel é escrito pelo autor da linguagem, com explicação e revisão.

**Atenção:** cada recurso que entra antes da 1.0 precisa ser implementado **duas vezes**: em Rust, e depois no `ksc.ks`. A linguagem até a 1.0 deve ter só o necessário.

## Decisões de design para as próximas versões

- **`struct` × `class`:** `struct` guarda só dados e é copiável. `class` tem métodos e `private`/`public`, e tem dono (`own`).
- **Classes (0.6), decidido em 2026-11-19:**
  - O método recebe o objeto como `self`, escrito nos parâmetros: `lent self` (usa) ou `own self` (consome). Método sem `self` é "static" e se chama pelo nome da classe: `Conta.nova()`.
  - Campos e métodos são `private` por padrão; o que for visível fora leva `public`.
  - O contrato se chama `interface`. A classe declara que o cumpre com `@impl(Interface)` em cima dela, e o compilador confere que todos os métodos existem com a assinatura certa.
  - Cada método que cumpre a interface leva `@override` (obrigatório, como um `@Override` do Java que não pode faltar). Um `@override` que não cumpre nada é erro (pega erros de digitação).
  - Campos podem ter dono (arrays, outros objetos); `burn c` libera tudo o que está dentro.
  - `Conta { ... }` só cria o objeto dentro da própria classe; fora dela se usa um método static (`Conta.nova()`), o "construtor" sem palavra especial.
  - `c.metodo()` empresta `c` (`lent self`); um método `own self` consome o objeto. `print(c)` mostra os campos.
  - Arrays de objetos (2026-11-20): o array é dono dos objetos, que entram com `give` e saem com `pop`/`remove` (que agora devolve o elemento). `v[i]` fica no array (emprestado). Sem elementos vazios: `claim`, `resize` e `clear` não valem para eles, e trocar `v[i] = ...` é erro. `burn v` libera cada objeto.
- **Antes do compilador (0.7), decidido em 2026-11-21:**
  - Null-safety como em Kotlin, para todos os tipos: um tipo sem `?` nunca é null; `int?`, `text?`, `No?` podem ser. Escreve-se `null`, `x == null`, `x != null` (sai o `isNaT`; `NaT` só no `on NaT`). O compilador prova o "não é null" (if, `&&`/`||`, `return` cedo, `assert`, atribuição) e exige a prova antes de usar. `a?.b` e `a?.m()`. Declarar sem valor exige `?`. `toInt`, `toReal` e `input` devolvem `?` em vez de disparar Trap.
  - Empréstimo com `&`, como em Rust (pedido do autor: `lend`/`lent` confundiam): `c: &Conta` e `&v[]: int` emprestam, `&self` empresta o objeto; sem `&` o parâmetro recebe a posse (`c: Conta`, `self`), e quem chama escreve `&c` ou `give c`. As palavras `lent`, `lend` e `own` saíram (dão erro com a forma nova).
  - Coringas (`? extends T`, `? super T`): considerados e **descartados** (2026-11-21). Com uma cópia do código por tipo, `proc f<T extends Forma>(fs: &List<T>)` chamado como `f<Circulo>(&lista)` faz o mesmo, e uma lista mista já é `List<Forma>`. Coringas obrigariam o compilador a deduzir o tipo pelo argumento (a única dedução da linguagem), criando cópias no meio da checagem.
  - Construtor com o nome da classe, como no Java, mas sem `new`: `public proc Conta(saldo: real) -> Conta`, chamado `Conta(10.0)` (genérico: `List<int>()`). Um por classe (não há sobrecarga); outras formas de criar são métodos estáticos com nome. Interfaces não têm construtor.
  - Feito em 2026-11-21: devolver empréstimo (`-> &T`, do primeiro parâmetro `&`), `x: &int` (copiado na entrada, escrito de volta na saída), `Object` automático (`toString`, `hashCode`, `equals`; `==` em objetos chama `equals`, `print` usa `toString`), `Comparable` sem `<T>` (como Swift/Rust: compara com a mesma classe; `<` em objetos e textos), `x ?: padrão`. `Comparable<T>` com interfaces genéricas ficou de fora: em `T extends Comparable<T>` o `T` aparece na própria restrição, o que a checagem dos genéricos não resolve hoje. `Rc` e ponteiros crus ficam para depois da 1.0.
  - Licença trocada de MIT para GPL v3 (ou posterior) em 2026-11-21.
  - Build (2026-11-21), sem arquivo de projeto (a estrutura das pastas diz tudo, como Java/C sem Gradle/CMake): `src/` (código; `Main.ks` é o principal e os `use` partem daqui), `lib/` (bibliotecas de terceiros como `.ksb`, como a pasta de `.jar` soltos), `build/obj/` (um `.ksb` por arquivo, cache) e `build/` (o executável). Pastas em minúscula, arquivos em PascalCase (`use jogo.Mapa;` lê `src/jogo/Mapa.ks`). A biblioteca padrão é `std`, por assunto (`std.text`, `std.collections`), embutida no `ksc` e sempre com `use` (nada entra sozinho). Subcomandos `ksc build`, `ksc run`, `ksc test`, `ksc new` para projetos; `ksc arquivo.ks` com flags para o uso avulso. O `.ksb` guarda as declarações do arquivo, então uma lib funciona sem o `.ks`. O compilador para no primeiro erro (até a 1.0).
  - Erros com `try`/`catch` entram na 0.7 (antes ficavam para depois da 1.0), no modelo do Java: `proc ler(p: text) -> Config throws NotFound, ParseError`, conferido pelo compilador (quem chama trata ou declara `throws`), chamadas sem marca. Lança-se um objeto de uma classe com `@impl(Error)` (`message(&self) -> text`); `catch e: NotFound` pega uma classe, `catch e: Error` todas. As falhas da VM (overflow, divisão por zero, índice, `assert`, null) viram o erro `Trap`, que não precisa de `throws` (como `RuntimeException`) e que `catch e: Trap` pega; sem `catch`, param o programa como hoje. Quando um erro sai de um bloco, o compilador libera sozinho o que ainda tem dono ali; o erro pego tem dono como qualquer objeto. Feito em 2026-11-21 (SPEC §8.17): uma tabela de tratadores por função no `.ksb` (versão 3), como a do `.class` do Java, e a instrução `THROW`.
  - Código de máquina (2026-11-21): o `ksc` em Rust ganha um gerador de código nativo antes da 1.0, para o `ksc build` dar um ELF com instruções x86-64 em vez da cópia do `ksvm` com o bytecode (hoje o executável já roda sem nada instalado, mas por dentro ainda é a VM). Só Linux x86-64 por enquanto; Windows fica para depois. O `.ksb` continua como objeto e para o `ksvm`. Versão a definir, depois do `try`/`catch`. Escolhido em 2026-11-21: assembler e linker próprios (sem `as`/`ld`). Etapa 1 feita no mesmo dia: o ELF é uma cópia do `ksvm` com um segmento a mais; saltos e chamadas são nativos, as outras instruções chamam o runtime (1,6× mais rápido que a VM). Etapa 2 feita no mesmo dia: mover valores, contas e comparações de `int` e desvios em x86 direto (11× mais rápido que a VM num laço com `fib`). Etapa 3 feita no mesmo dia: `/` e `%`, e a pilha adiada (variáveis, constantes e o último resultado só vão para a pilha da VM quando preciso): 76× mais rápido que a VM num laço com `%`. Próximas: chamadas de função sem passar pelo runtime, e arrays.
  - Arquivos e `use` (2026-11-21), modelo do `gcc` com o `.h` gerado sozinho: `use Geometry;` / `use Shapes.Circle;`, nomes de arquivo PascalCase, itens de outro arquivo sempre com prefixo (`Geometry.area()`), o item com o nome do arquivo vale pelo nome do arquivo (`List<int>()` de `List.ks`), só o `public` sai do arquivo. Ciclos permitidos (cada arquivo compila lendo só as declarações dos outros, como os `.h` do C, sem escrevê-los à mão). Um `.ksb` por arquivo em `build/`, recompilado só quando ele ou a parte pública do que ele usa muda; o ligador junta os objetos e escreve as chamadas por interface. O resultado é o executável (cópia do `ksvm` com o programa); `--ksb` dá o `.ksb`. O arquivo principal não pode ser usado (como o `package main` do Go). Comparado: o Rust compila um crate inteiro junto (ciclos só dentro do crate), o Go proíbe ciclos.
  - Trit 0 numa condição continua disparando Trap (para tratar os três casos existe `tri`).
  - Variável emprestada: `lent atual: No? = lend lista;` percorre listas e árvores com laços; o empréstimo vale até o último uso, e enquanto isso o dono não pode ser queimado, entregue, esvaziado nem emprestado. O `burn` de uma lista ligada também virou laço.
  - Genéricos, como no Java na escrita: `class Box<T>`, `proc max<T extends Comparable>(a: T, b: T) -> T`, o tipo sempre escrito (`List<int>.new()`, `max<int>(3, 7)`), coringas `? extends T` e `? super T`. `T` pode ser qualquer tipo, inclusive `int` (não há caixas como `Integer`) e objetos com dono. Por dentro, um código por tipo usado (como C++/Rust), não *type erasure*: os valores têm tamanhos diferentes.
  - Sem herança até a 1.0 (decidido em 2026-11-21; depois da 1.0 o autor decide, ou se alguém pedir). No lugar: métodos padrão em interfaces (como no Java 8) e `Object` como uma interface que todo tipo cumpre sozinho (`equals`, `hashCode`, `toString`), então um `<T>` sem `extends` já compara e imprime. Todo código nativo (biblioteca, built-ins) tem nomes em inglês.
  - Tipos recursivos (2026-11-20), escolhido o modelo do Kotlin/Swift: `filhos[]: No` (árvores) e campos opcionais `proximo: No?` (vazio = NaT). O compilador obriga a conferir com `isNaT` antes de usar, e a conferência se perde quando algo pode ter esvaziado o campo. `give n.proximo` tira o objeto; atribuir só a um `T?` vazio (senão Trap). `burn` usa uma função escondida por classe, então libera árvores de qualquer profundidade.
  - Voltar da interface para a classe (2026-11-20): `f is Circulo` (como em Kotlin, não `==`) e `match f { Circulo => ...; }`; onde provado, o próprio `f` vira `Circulo` (sem nome novo por ramo, para não haver dois nomes para o mesmo objeto).
  - Interfaces com dono (2026-11-20): variáveis, campos, `own`, retorno e arrays de uma interface guardam objetos de classes diferentes; `burn` libera conforme a classe.
  - Um objeto de classe tem dono, como um array: `burn`, `give`, `lend` e `return`, e esquecer é erro de compilação.
- **Sem herança:** reaproveitamento por composição (uma classe tem outra como campo) e por interfaces, como em Go e Rust. Herança conflita com a posse (quem é dono da parte herdada? o que o `burn` libera?).
- **Construtor (2026-11-21):** o método estático com o nome da classe (`Conta(...)`), que cria o objeto preenchendo os campos: `Token { kind: 1, start: 0, len: 4 }`.
- **Anotações** são sempre minúsculas (`@impl`) e só são conferidas na compilação, como em Java.
- **Sem toques de assembly** (removidos na 0.3: `@R`, `add d, s;`, `tri cmp3`), porque eram só outra forma de escrever. Se um dia for preciso acesso real às instruções da ksvm, a ideia é um bloco `asm { }` isolado, depois da 1.0.
- **Dois tipos de array** (0.3): `x[]: int` com índices `0..N-1` e `sym x[]: int` com índices `-k..+k`. Os colchetes ficam no nome; `{1, 2, 3}` é o literal; parâmetros usam `lent`/`own` como modificador.
- **Hash map e hash set** são biblioteca escrita em Kestrel, não parte da linguagem.
- **Lista = o próprio array** (0.5): como não há genéricos, uma `List` na biblioteca teria que ser escrita uma vez por tipo. Por isso o array tem `append`, `pop` etc. embutidos, para qualquer tipo de elemento.
- **Primitivas × biblioteca:** só vira instrução da ksvm o que a linguagem não consegue fazer sozinha (arquivos, stdin, argumentos, hora, número aleatório). Todo o resto (math, logging, lista, hash map, funções de texto) é biblioteca escrita em Kestrel.
- **A biblioteca padrão é feita em duas fases:**
  - **Antes da 1.0:** só o que o `ksc.ks` usa: hash map, montar textos, ler arquivo, erros com mensagem.
  - **Depois da 1.0:** math completo, logging, data e hora, números aleatórios e o que mais aparecer.
- **Erros:** uma conta que falha (overflow, divisão por zero, `toInt` inválido) dispara Trap com o motivo, e `on NaT` trata o erro na própria linha. O erro que sobe de um proc para quem chamou é o `try`/`catch` da 0.7 (veja as decisões acima).
- **`.ksb` legível, como o `.class` do Java (0.7), ideia de 2026-11-19:**
  - **Seção de símbolos:** o `.ksb` guarda tudo o que é público, com os tipos: `proc area(lent self) -> real`, campos, `struct`, `enum`, `class` e `interface`. Com ela, `use geometria;` confere os tipos lendo só o `geometria.ksb`, sem o `.ks`, como o `javac` lê um `.class`.
  - **Tabela de linhas:** cada instrução aponta para a linha do `.ks` que a gerou. Serve para o traceback e para o editor.
  - **Documentação, como no Java:** `//` continua sendo comentário comum, que some na compilação. Um comentário `/* ... */` logo acima de um item público é a documentação dele: vai para a seção de símbolos do `.ksb` e aparece no hover do editor e na visualização, como o Javadoc.
  - **Documentação do módulo:** o primeiro `/* */` do arquivo, seguido de uma linha em branco, documenta o módulo inteiro (como o `package-info.java`). Passar o mouse em `use time;` mostra esse texto.
  - **Formato da documentação:** Markdown (títulos, negrito, listas, tabelas, blocos de código), como no Java 23 (JEP 467), no Rust e no Kotlin, porque o hover do VSCode já é Markdown. `{@link alvo}` vira um link para a definição. No fim vêm as tags `@since`, `@author`, `@param`, `@return` e `@deprecated`.

    ```
    /*
     * Data e hora: ler o relógio, somar dias e formatar datas.
     *
     * - **Fuso:** sempre UTC.
     * - Para intervalos, veja {@link duracao}.
     *
     * @since 1.0
     * @author Edynu
     */
    ```
- **Onde cada coisa mora (0.7), ideia de 2026-11-19:**

  | Camada | Exemplo | Onde mora |
  |---|---|---|
  | Primitivas | `print`, `len`, `append`, `readFile` | Instruções do `ksvm`, como o `libjvm` do Java |
  | Biblioteca padrão | hash map, funções de texto | `.ksb` embutidos dentro do `ksc`/`ksvm` (`include_bytes!`), como o `lib/modules` do JDK |
  | Bibliotecas externas | a lib de outra pessoa | Arquivos `.ksb`, como os `.jar` |

  `use texto;` procura primeiro dentro do executável e depois nos arquivos do projeto.
- **Executável único (0.7):** o `.ksb` continua sendo o formato de biblioteca e de módulo, mas para distribuir o `ksc` gera um executável só: uma cópia do `ksvm` com os `.ksb` usados grudados no fim, e uma marca dizendo onde começam. Quem recebe roda com dois cliques, sem instalar uma VM (diferente do `.jar`, que exige o JRE). Vai junto o *runtime*, não o compilador, como no Go. Depois da 1.0, o próprio `ksc.ksb` vira o executável `ksc` assim.
- **Cabeçalho de licença:** todo arquivo da biblioteca padrão e do `ksc.ks` começa com
  ```
  // Copyright (c) 2026 Edynu
  // SPDX-License-Identifier: MIT
  ```
  Como é `//`, não vira documentação.
  - **Visualização só leitura:** abrir um `.ksb` (ou dar F12 num item importado) mostra as assinaturas, com a documentação e o corpo como `/* compilado */`, e o bytecode (disassembly) logo abaixo. No topo vai um aviso: `// Gerado a partir de geometria.ksb pelo ksc. Somente leitura.`
  - Opcionalmente, o `.ksb` pode embutir o `.ks` original (como o `-sources.jar`). Aí a visualização mostra o código exato.
- **Orientação a dados:** a árvore do compilador fica num buffer de nós ligados por índice, sem ponteiros dentro de dados.

## Como cada parte do `ksc.ks` é validada

Cada parte é comparada com o `ksc` em Rust, que serve de referência. Para os mesmos programas de teste, as duas versões precisam produzir a mesma saída: a mesma lista de tokens, a mesma árvore, os mesmos erros e o mesmo `.ksb`, byte a byte.

## Teste da 1.0 (ponto fixo)

1. O `ksc` em Rust compila `ksc.ks` e gera `ksc1.ksb`.
2. `ksvm ksc1.ksb ksc.ks` gera `ksc2.ksb`.
3. `ksvm ksc2.ksb ksc.ks` gera `ksc3.ksb`.
4. `ksc2.ksb` e `ksc3.ksb` são **idênticos byte a byte**.

Depois da 1.0, o `ksc` em Rust continua no repositório como referência e para iniciar o bootstrap numa máquina nova.

## Depois da 1.0

- Biblioteca padrão completa: math, logging, data e hora, números aleatórios.
- Bloco `asm { }` para acesso direto às instruções da ksvm.
- Descompilador completo: reconstruir `if`/`while`/`tri` a partir dos saltos do bytecode, como o FernFlower.

## Versões e tags

Cada versão lançada atualiza o `version` do `Cargo.toml` e recebe uma tag `vX.Y.Z`.
