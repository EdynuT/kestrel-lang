proc main() {
    // prints
    print("Meu nome e Edynu\n");
    print("Estou aprendendo Kestrel\n");

    // declaração de variaveis
    variavel1: int = 5;
    printf("exemplo de %%t = %t = 5\n", variavel1);
    printf("exemplo de %%v = %v = 5\n", variavel1);
    variavel1 = 15;
    printf("\n%i\n", variavel1);

    // operações
    final variavel2: int = 2;
    printl(variavel2 + 2);
    printl(variavel2 - 2);
    printl(variavel2 * 2);
    printl(variavel2 / 2);

    variavel3: int = variavel2 + variavel1;
    printl(variavel3);
    variavel3: int = variavel2 - variavel1;
    printl(variavel3);
    variavel3: int = variavel2 * variavel1;
    printl(variavel3);
    variavel3: real = (real) variavel2 / (real) variavel1;
    printl(variavel3);

    if variavel1 < variavel2 {
        printf("{variavel1} é menor que {variavel2}\n");
    } else if (real) variavel2 < variavel3 {
        printf("{variavel2} é maior que {variavel1} e menor que {variavel3}\n");
    } else if (real) variavel1 < variavel3 {
        printf("{variavel1} é menor que {variavel3}\n");
    } else {
        printf("{variavel3} é minusculo\n");
    }

    a: int = 0;
    while a < 10 {
        printl(a);
        a++;
    }
}
